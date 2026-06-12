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
    /// Weight of the log-frequency prior.
    pub beta: f32,
    /// Pruning radius (keyboard units) for the start/end gate.
    pub prune_radius: f32,
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
            beta: 0.25,
            prune_radius: 1.8,
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
}

impl Decoder {
    /// Build a decoder, precomputing a template for every gestureable word.
    pub fn new(layout: KeyboardLayout, dict: Dictionary, params: DecoderParams) -> Self {
        let templates = (0..dict.len())
            .filter_map(|i| {
                Template::build(i, dict.word(i), dict.ln_freq(i), &layout, params.n)
            })
            .collect();
        Decoder {
            layout,
            dict,
            templates,
            params,
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
        let rs = input.resample(p.n);
        let loc_in: Vec<[f32; 2]> = rs.points.iter().map(|pt| [pt.x, pt.y]).collect();
        let shape_in = normalize_shape(&rs);
        let start = loc_in[0];
        let end = loc_in[loc_in.len() - 1];
        let r2 = p.prune_radius * p.prune_radius;

        let shape_k = 1.0 / (2.0 * p.shape_sigma * p.shape_sigma);
        let loc_k = 1.0 / (2.0 * p.loc_sigma * p.loc_sigma);

        let mut out: Vec<Candidate> = Vec::new();
        for t in &self.templates {
            // Pruning gate: the swipe must begin near the first key and end near
            // the last key of the candidate word.
            if dist2(start, t.start) > r2 || dist2(end, t.end) > r2 {
                continue;
            }
            let shape_dist = mean_dist(&shape_in, &t.shape);
            let loc_dist = mean_dist(&loc_in, &t.loc);
            let score = -(shape_dist * shape_dist) * shape_k - (loc_dist * loc_dist) * loc_k
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{human_like, sample_stroke, Rng};
    use crate::template::ideal_trace;

    fn decoder() -> Decoder {
        Decoder::with_defaults(KeyboardLayout::qwerty(), Dictionary::english())
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

    /// The headline metric: top-1 / top-3 accuracy on perturbed synthetic
    /// swipes. Tune DecoderParams against this.
    #[test]
    fn accuracy_under_moderate_perturbation() {
        let dec = decoder();
        let (p1, p3, total) = measure(&dec, 0.45, 4, 0xC0FFEE_1234);
        println!("accuracy: top1={p1:.3} top3={p3:.3} over {total} trials");
        assert!(p1 > 0.90, "top-1 accuracy regressed: {p1:.3}");
        assert!(p3 > 0.99, "top-3 accuracy regressed: {p3:.3}");
    }
}
