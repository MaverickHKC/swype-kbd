//! The SHARK2-style decoder: rank dictionary words for a captured swipe by
//! combining a shape channel, a location channel, and a frequency prior, after
//! a cheap start/end pruning gate (ARCHITECTURE.md §4).

use crate::dictionary::Dictionary;
use crate::layout::KeyboardLayout;
use crate::template::{normalize_shape, Template};
use crate::trace::Trace;

const EPS: f32 = 1e-6;

/// Tunable decoder parameters.
#[derive(Debug, Clone)]
pub struct DecoderParams {
    /// Resample resolution for both channels.
    pub n: usize,
    /// Gaussian width of the shape channel (normalized-space units).
    pub shape_sigma: f32,
    /// Gaussian width of the location channel (keyboard units).
    pub loc_sigma: f32,
    /// Gaussian width of the endpoint channel (keyboard units). Tighter than
    /// `loc_sigma`: the first and last points are placed deliberately, so they
    /// disambiguate words that differ only at the ends (e.g. hell-o vs hel-p).
    pub end_sigma: f32,
    /// Weight of the log-frequency prior.
    pub beta: f32,
    /// Pruning radius (keyboard units) for the start/end gate.
    pub prune_radius: f32,
    /// Passes of endpoint-preserving moving-average smoothing applied to the
    /// captured trace before resampling, to absorb finger/pointer jitter.
    pub smooth_passes: usize,
    /// Velocity weighting of the location channel, in `[0, 1]`. Real swipes slow
    /// down through the keys they mean to hit and speed across the gaps; with the
    /// trace resampled equidistantly in space, the time between samples measures
    /// that dwell. At 0 every point counts equally (plain mean); at 1 each point
    /// is weighted by its dwell time, so deliberate points dominate the match.
    pub vel_weight: f32,
    /// Maximum candidates returned (top-1 plus alternates).
    pub max_candidates: usize,
}

impl Default for DecoderParams {
    fn default() -> Self {
        // Tuned against the synthetic accuracy harness (see `param_sweep`).
        // A tight location channel plus a modest frequency prior leans on
        // geometry, which disambiguates similar shapes best.
        DecoderParams {
            n: 100,
            shape_sigma: 0.18,
            loc_sigma: 0.55,
            end_sigma: 0.45,
            beta: 0.25,
            prune_radius: 1.8,
            // Off by default: the synthetic harness (`smooth_sweep`) shows the
            // Gaussian channels already absorb mild jitter, so decode-side
            // smoothing only rounds corners. Left tunable for real-device input,
            // which carries spikier noise the synthetic model doesn't capture.
            smooth_passes: 0,
            // A modest dwell weighting: on the realistic synthetic velocity
            // profile (`vel_sweep`) 0.5 never regresses and edges top-1 up,
            // while 1.0 over-weights and hurts clean swipes. Real fingers dwell
            // on keys more sharply than the conservative synthetic model, so the
            // on-device gain should exceed the synthetic margin.
            vel_weight: 0.5,
            max_candidates: 8,
        }
    }
}

/// A ranked decode result.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub word: String,
    /// Combined log-score; higher is better.
    pub score: f32,
    pub shape_dist: f32,
    pub loc_dist: f32,
}

/// A decoder over a fixed layout + dictionary, with precomputed templates.
pub struct Decoder {
    layout: KeyboardLayout,
    dict: Dictionary,
    templates: Vec<Template>,
    params: DecoderParams,
    /// Lowercase word -> index into `templates`, for `learn`.
    index: std::collections::HashMap<String, usize>,
}

impl Decoder {
    /// Build a decoder, precomputing a template for every gestureable word.
    pub fn new(layout: KeyboardLayout, dict: Dictionary, params: DecoderParams) -> Self {
        let templates: Vec<Template> = (0..dict.len())
            .filter_map(|i| {
                Template::build(i, dict.word(i), dict.ln_freq(i), &layout, params.n)
            })
            .collect();
        let index = templates
            .iter()
            .enumerate()
            .map(|(i, t)| (dict.word(t.word_index).to_string(), i))
            .collect();
        Decoder {
            layout,
            dict,
            templates,
            params,
            index,
        }
    }

    /// Top `max` dictionary words beginning with `prefix`, ranked by (learned)
    /// frequency — the predictive completions for tap typing. The prefix itself,
    /// if it is a word, is included so it can be committed with a space. Case-
    /// insensitive; returns lowercase dictionary words.
    pub fn complete(&self, prefix: &str, max: usize) -> Vec<String> {
        if prefix.is_empty() || max == 0 {
            return Vec::new();
        }
        let p = prefix.to_ascii_lowercase();
        let mut hits: Vec<(&str, f32)> = self
            .templates
            .iter()
            .filter_map(|t| {
                let w = self.dict.word(t.word_index);
                if w.starts_with(&p) {
                    Some((w, t.ln_freq))
                } else {
                    None
                }
            })
            .collect();
        hits.sort_by(|a, b| b.1.total_cmp(&a.1));
        hits.truncate(max);
        hits.into_iter().map(|(w, _)| w.to_string()).collect()
    }

    /// Whether `word` is a known (gestureable) dictionary word.
    pub fn contains(&self, word: &str) -> bool {
        self.index.contains_key(&word.to_ascii_lowercase())
    }

    /// The (learned) log-frequency of a word, or `None` if it has no template.
    /// Comparable across words: higher means more frequent.
    pub fn ln_freq_of(&self, word: &str) -> Option<f32> {
        self.index
            .get(&word.to_ascii_lowercase())
            .map(|&i| self.templates[i].ln_freq)
    }

    /// Adjust a word's log-frequency prior by `delta` (per-user learning).
    /// Returns true if the word has a template. The change affects all future
    /// `decode` calls; persistence is the caller's responsibility.
    pub fn learn(&mut self, word: &str, delta: f32) -> bool {
        match self.index.get(&word.to_ascii_lowercase()) {
            Some(&i) => {
                self.templates[i].ln_freq += delta;
                true
            }
            None => false,
        }
    }

    pub fn with_defaults(layout: KeyboardLayout, dict: Dictionary) -> Self {
        Self::new(layout, dict, DecoderParams::default())
    }

    pub fn layout(&self) -> &KeyboardLayout {
        &self.layout
    }

    pub fn dictionary(&self) -> &Dictionary {
        &self.dict
    }

    pub fn params(&self) -> &DecoderParams {
        &self.params
    }

    /// Number of precomputed templates (gestureable words).
    pub fn templates_len(&self) -> usize {
        self.templates.len()
    }

    /// Rank candidates for a captured trace, best first. Empty if the trace is
    /// too short to be a gesture.
    pub fn decode(&self, input: &Trace) -> Vec<Candidate> {
        if input.len() < 2 || input.path_length() <= EPS {
            return Vec::new();
        }
        let p = &self.params;
        let rs = if p.smooth_passes > 0 {
            input.smoothed(p.smooth_passes).resample(p.n)
        } else {
            input.resample(p.n)
        };
        let loc_in: Vec<[f32; 2]> = rs.points.iter().map(|pt| [pt.x, pt.y]).collect();
        let shape_in = normalize_shape(&rs);
        let weights = dwell_weights(&rs, p.vel_weight);
        let start = loc_in[0];
        let end = loc_in[loc_in.len() - 1];
        let r2 = p.prune_radius * p.prune_radius;

        let shape_k = 1.0 / (2.0 * p.shape_sigma * p.shape_sigma);
        let loc_k = 1.0 / (2.0 * p.loc_sigma * p.loc_sigma);
        let end_k = 1.0 / (2.0 * p.end_sigma * p.end_sigma);

        let mut out: Vec<Candidate> = Vec::new();
        for t in &self.templates {
            // Pruning gate: the swipe must begin near the first key and end near
            // the last key of the candidate word. Reuse these squared distances
            // as the endpoint channel.
            let d_start = dist2(start, t.start);
            let d_end = dist2(end, t.end);
            if d_start > r2 || d_end > r2 {
                continue;
            }
            let shape_dist = mean_dist(&shape_in, &t.shape);
            let loc_dist = weighted_mean_dist(&loc_in, &t.loc, &weights);
            let score = -(shape_dist * shape_dist) * shape_k
                - (loc_dist * loc_dist) * loc_k
                - (d_start + d_end) * end_k
                + p.beta * t.ln_freq;
            out.push(Candidate {
                word: self.dict.word(t.word_index).to_string(),
                score,
                shape_dist,
                loc_dist,
            });
        }

        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        out.truncate(p.max_candidates);
        out
    }
}

#[inline]
fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy
}

/// Mean Euclidean distance between two equal-length point sequences.
fn mean_dist(a: &[[f32; 2]], b: &[[f32; 2]]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let n = a.len().max(1) as f32;
    let sum: f32 = a
        .iter()
        .zip(b)
        .map(|(p, q)| dist2(*p, *q).sqrt())
        .sum();
    sum / n
}

/// Weighted mean Euclidean distance between two equal-length point sequences.
/// With all-equal weights this is exactly [`mean_dist`].
fn weighted_mean_dist(a: &[[f32; 2]], b: &[[f32; 2]], w: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    debug_assert_eq!(a.len(), w.len());
    let mut num = 0.0f32;
    let mut den = 0.0f32;
    for ((p, q), &wi) in a.iter().zip(b).zip(w) {
        num += wi * dist2(*p, *q).sqrt();
        den += wi;
    }
    num / den.max(EPS)
}

/// Per-point weights for the location channel from a trace resampled
/// equidistantly in space: the local time between samples is the dwell time on
/// that fixed arc step, so slow (deliberate) points get more weight. `vel_weight`
/// in `[0, 1]` blends from uniform (0) to fully dwell-proportional (1). Falls
/// back to uniform when there is no usable timing (templates, zero-duration).
fn dwell_weights(rs: &Trace, vel_weight: f32) -> Vec<f32> {
    let n = rs.points.len();
    if vel_weight <= 0.0 || n < 2 {
        return vec![1.0; n];
    }
    let t: Vec<f32> = rs.points.iter().map(|p| p.t as f32).collect();
    if t[n - 1] - t[0] <= EPS {
        return vec![1.0; n];
    }
    // Time per arc-step around the point (the centered window spans 2 steps in
    // the interior, 1 at the ends). Dividing by the step count yields inverse
    // speed, so a constant-speed trace weights uniformly and only genuine
    // slow-downs stand out — no spurious endpoint bias from the half-window.
    let dwell: Vec<f32> = (0..n)
        .map(|i| {
            let lo = i.saturating_sub(1);
            let hi = (i + 1).min(n - 1);
            (t[hi] - t[lo]).max(0.0) / (hi - lo) as f32
        })
        .collect();
    let mean = dwell.iter().sum::<f32>() / n as f32;
    if mean <= EPS {
        return vec![1.0; n];
    }
    dwell
        .iter()
        .map(|d| (1.0 - vel_weight) + vel_weight * (d / mean))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{human_like, sample_stroke, Rng};
    use crate::template::ideal_trace;

    fn decoder() -> Decoder {
        Decoder::with_defaults(KeyboardLayout::qwerty(), Dictionary::english())
    }

    #[test]
    fn dwell_weights_are_uniform_without_velocity_signal_or_when_off() {
        use crate::trace::Point;
        // Constant-speed trace (linear timestamps): no dwell signal -> uniform.
        let rs = Trace::from_points(
            (0..5).map(|i| Point::new(i as f32, 0.0, (i * 10) as u32)).collect(),
        );
        let w = dwell_weights(&rs, 0.5);
        assert!(w.iter().all(|&x| (x - 1.0).abs() < 1e-5), "uniform speed -> {w:?}");
        // A slow first segment (100 ms) then a fast one (10 ms), equal spacing.
        let rs2 = Trace::from_points(vec![
            Point::new(0.0, 0.0, 0),
            Point::new(1.0, 0.0, 100),
            Point::new(2.0, 0.0, 110),
        ]);
        // vel_weight 0 is always uniform, even with a dwell.
        assert_eq!(dwell_weights(&rs2, 0.0), vec![1.0; 3]);
        // With weighting on, weight tracks slowness: highest where the bordering
        // segments are slow, lowest in the fast stretch.
        let w2 = dwell_weights(&rs2, 1.0);
        assert!(w2[0] > w2[1] && w2[1] > w2[2], "weight should follow slowness: {w2:?}");
        // weighted_mean_dist reduces to mean_dist under uniform weights.
        let a = [[0.0, 0.0], [1.0, 0.0]];
        let b = [[0.0, 1.0], [1.0, 2.0]];
        assert!((weighted_mean_dist(&a, &b, &[1.0, 1.0]) - mean_dist(&a, &b)).abs() < 1e-6);
    }

    #[test]
    fn ideal_trace_decodes_to_its_word() {
        let kb = KeyboardLayout::qwerty();
        let dec = decoder();
        for w in ["work", "people", "about", "because", "world"] {
            let trace = sample_stroke(w, &kb, 64).unwrap();
            let cands = dec.decode(&trace);
            assert_eq!(cands[0].word, w, "ideal trace for {w} -> {:?}", cands[0]);
        }
    }

    #[test]
    fn learning_boosts_a_word_up_the_ranking() {
        let kb = KeyboardLayout::qwerty();
        let mut dec = decoder();
        // Find a swipe whose top candidate is not some target alternate, then
        // boost that alternate and confirm it climbs.
        let trace = sample_stroke("that", &kb, 64).unwrap();
        let before = dec.decode(&trace);
        assert_eq!(before[0].word, "that");
        // Pick the runner-up and boost it hard.
        let alt = before[1].word.clone();
        assert!(dec.learn(&alt, 50.0));
        let after = dec.decode(&trace);
        assert_eq!(after[0].word, alt, "boosted word should now rank first");
        // Unknown words are a no-op.
        assert!(!dec.learn("zzzzqx", 1.0));
    }

    #[test]
    fn completion_ranks_by_frequency_and_respects_prefix() {
        let mut dec = decoder();
        let comps = dec.complete("th", 5);
        assert!(!comps.is_empty());
        assert!(comps.iter().all(|w| w.starts_with("th")));
        // "the" is the most frequent th- word in the embedded list.
        assert_eq!(comps[0], "the");
        // Learning lifts a completion.
        let target = "this".to_string();
        assert!(dec.learn(&target, 50.0));
        let comps2 = dec.complete("th", 5);
        assert_eq!(comps2[0], "this");
        // Empty prefix -> nothing.
        assert!(dec.complete("", 5).is_empty());
    }

    #[test]
    fn empty_or_tiny_trace_returns_nothing() {
        let dec = decoder();
        assert!(dec.decode(&Trace::new()).is_empty());
        let one = sample_stroke("the", &KeyboardLayout::qwerty(), 64).unwrap();
        let single = Trace::from_points(vec![one.points[0]]);
        assert!(dec.decode(&single).is_empty());
    }

    #[test]
    fn templates_skip_ungestureable_words() {
        let dec = decoder();
        // Every template should be < dict size (single-letter words dropped).
        assert!(dec.templates_len() <= dec.dictionary().len());
        assert!(dec.templates_len() > 200);
    }

    /// Measure top-1/top-3 for a given decoder over `trials` perturbed swipes
    /// per word at a given perturbation level. Shared by the accuracy test and
    /// the tuning sweep.
    #[cfg(test)]
    fn measure(dec: &Decoder, level: f32, trials: usize, seed: u64) -> (f32, f32, u32) {
        let kb = KeyboardLayout::qwerty();
        let words: Vec<String> = dec
            .dictionary()
            .words()
            .iter()
            .filter(|w| w.chars().count() >= 3 && ideal_trace(w, &kb).is_some())
            .take(150)
            .cloned()
            .collect();
        let mut rng = Rng::new(seed);
        let (mut top1, mut top3, mut total) = (0u32, 0u32, 0u32);
        for w in &words {
            for _ in 0..trials {
                let trace = human_like(w, &kb, &mut rng, level).unwrap();
                let cands = dec.decode(&trace);
                total += 1;
                if cands.first().map(|c| &c.word == w).unwrap_or(false) {
                    top1 += 1;
                }
                if cands.iter().take(3).any(|c| &c.word == w) {
                    top3 += 1;
                }
            }
        }
        (top1 as f32 / total as f32, top3 as f32 / total as f32, total)
    }

    /// Velocity-weighting sweep behind `vel_weight`, on the curvature-aware
    /// synthetic velocity profile (`synth::stamp_velocity`). Run with:
    /// `cargo test -p swype-decoder vel_sweep -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn vel_sweep() {
        let kb = KeyboardLayout::qwerty();
        // A harder set than `measure`: longer words (≥5 letters) have more
        // interior keys to dwell on and are more confusable, so the top-150
        // common-word ceiling doesn't hide the effect.
        let words: Vec<String> = Dictionary::english()
            .words()
            .iter()
            .filter(|w| w.chars().count() >= 5 && ideal_trace(w, &kb).is_some())
            .take(250)
            .cloned()
            .collect();
        for level in [0.7f32, 1.0] {
            for vel_weight in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
                let params = DecoderParams {
                    vel_weight,
                    ..Default::default()
                };
                let dec = Decoder::new(kb.clone(), Dictionary::english(), params);
                let mut rng = Rng::new(0xC0FFEE_1234);
                let (mut top1, mut top3, mut total) = (0u32, 0u32, 0u32);
                for w in &words {
                    for _ in 0..8 {
                        let trace = human_like(w, &kb, &mut rng, level).unwrap();
                        let cands = dec.decode(&trace);
                        total += 1;
                        top1 += cands.first().map_or(0, |c| (&c.word == w) as u32);
                        top3 += cands.iter().take(3).any(|c| &c.word == w) as u32;
                    }
                }
                let (p1, p3) = (top1 as f32 / total as f32, top3 as f32 / total as f32);
                println!("level={level:.2} vel={vel_weight:.2} -> top1={p1:.3} top3={p3:.3} (n={total})");
            }
        }
    }

    /// Trace-smoothing sweep behind the off-by-default `smooth_passes`. Run with:
    /// `cargo test -p swype-decoder smooth_sweep -- --ignored --nocapture`.
    /// On the synthetic harness more passes only cost accuracy (the Gaussian
    /// channels already absorb the jitter); kept to re-measure against spikier
    /// real-device captures before enabling it.
    #[test]
    #[ignore]
    fn smooth_sweep() {
        let kb = KeyboardLayout::qwerty();
        for level in [0.45f32, 0.7, 1.0] {
            for smooth_passes in [0usize, 1, 2, 3] {
                let params = DecoderParams {
                    smooth_passes,
                    ..Default::default()
                };
                let dec = Decoder::new(kb.clone(), Dictionary::english(), params);
                let (p1, p3, _) = measure(&dec, level, 6, 0xC0FFEE_1234);
                println!("level={level:.2} smooth={smooth_passes} -> top1={p1:.3} top3={p3:.3}");
            }
        }
    }

    /// Reproducible parameter sweep. Run with:
    /// `cargo test -p swype-decoder param_sweep -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn param_sweep() {
        let kb = KeyboardLayout::qwerty();
        for shape_sigma in [0.18f32, 0.22, 0.26] {
            for loc_sigma in [0.55f32, 0.70, 0.85] {
                for beta in [0.25f32, 0.35, 0.5] {
                    let params = DecoderParams {
                        shape_sigma,
                        loc_sigma,
                        beta,
                        ..Default::default()
                    };
                    let dec = Decoder::new(kb.clone(), Dictionary::english(), params);
                    let (p1, p3, _) = measure(&dec, 0.45, 4, 0xC0FFEE_1234);
                    println!(
                        "shape={shape_sigma:.2} loc={loc_sigma:.2} beta={beta:.2} -> top1={p1:.3} top3={p3:.3}"
                    );
                }
            }
        }
    }

    /// Latency + accuracy against the real ~50k unigram list (milestone 5).
    /// Skipped unless `data/words_50k.txt` exists at the workspace root. Run:
    /// `cargo test -p swype-decoder latency_50k -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn latency_50k() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/words_50k.txt");
        let Ok(text) = std::fs::read_to_string(path) else {
            println!("skip: {path} not found");
            return;
        };
        let kb = KeyboardLayout::qwerty();
        let dict = Dictionary::parse_counts(&text);
        let t0 = std::time::Instant::now();
        let dec = Decoder::with_defaults(kb.clone(), dict);
        println!(
            "built {} templates in {} ms",
            dec.templates_len(),
            t0.elapsed().as_millis()
        );

        // Accuracy over common words (top of the list), then latency timing.
        let (p1, p3, total) = measure(&dec, 0.45, 4, 0xBEEF_5050);
        println!("50k accuracy (top-150 common words): top1={p1:.3} top3={p3:.3} n={total}");

        let words: Vec<String> = dec
            .dictionary()
            .words()
            .iter()
            .filter(|w| w.chars().count() >= 3 && ideal_trace(w, &kb).is_some())
            .take(500)
            .cloned()
            .collect();
        let mut rng = Rng::new(0x1234_5050);
        let mut times_us: Vec<u128> = Vec::new();
        for w in &words {
            let trace = human_like(w, &kb, &mut rng, 0.45).unwrap();
            let t = std::time::Instant::now();
            let _ = dec.decode(&trace);
            times_us.push(t.elapsed().as_micros());
        }
        times_us.sort_unstable();
        let mean = times_us.iter().sum::<u128>() as f64 / times_us.len() as f64;
        let p50 = times_us[times_us.len() / 2];
        let p99 = times_us[times_us.len() * 99 / 100];
        let max = *times_us.last().unwrap();
        println!(
            "decode latency over {} swipes: mean={:.0}us p50={p50}us p99={p99}us max={max}us",
            times_us.len(),
            mean
        );
        assert!(p99 < 30_000, "p99 decode {p99}us exceeds 30ms budget");
    }

    /// The headline metric: top-1 / top-3 accuracy on perturbed synthetic
    /// swipes. Tune DecoderParams against this.
    #[test]
    fn accuracy_under_moderate_perturbation() {
        let dec = decoder();
        let (p1, p3, total) = measure(&dec, 0.45, 4, 0xC0FFEE_1234);
        println!("accuracy: top1={p1:.3} top3={p3:.3} over {total} trials");
        assert!(p1 > 0.96, "top-1 accuracy regressed: {p1:.3}");
        assert!(p3 > 0.99, "top-3 accuracy regressed: {p3:.3}");
    }
}
