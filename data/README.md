# Dictionary data

## `words_50k.txt`

`word count` per line, whitespace-separated, ordered by descending frequency.
Loaded at runtime by `swype-kbd` (see `load_dictionary` / `dict_paths` in the
app) and turned into the decoder's frequency prior via
`Dictionary::parse_counts`.

**Provenance.** The top 50,000 purely-lowercase words (length 2–20), taken in
frequency order from Peter Norvig's `count_1w.txt` unigram list (the Google Web
Trillion Word Corpus counts), <https://norvig.com/ngrams/>. Regenerate with:

```sh
curl -sS -o /tmp/count_1w.txt https://norvig.com/ngrams/count_1w.txt
awk -F'\t' '$1 ~ /^[a-z]{2,20}$/ {print $1" "$2; c++} c>=50000{exit}' \
    /tmp/count_1w.txt > data/words_50k.txt
```

Override the path with the `SWYPE_DICT` environment variable to use a different
list (any `word count` file works).
