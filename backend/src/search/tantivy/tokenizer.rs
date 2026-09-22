use tantivy::tokenizer::{LowerCaser, NgramTokenizer, TextAnalyzer};

pub const TOKENIZER_NAME: &str = "rain_ngram_v2";
pub const NGRAM_MIN: usize = 3;
pub const NGRAM_MAX: usize = 3;

pub fn analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(
        NgramTokenizer::all_ngrams(NGRAM_MIN, NGRAM_MAX)
            .expect("valid Tantivy n-gram tokenizer bounds"),
    )
    .filter(LowerCaser)
    .build()
}

#[cfg(test)]
mod tests {
    use super::{NGRAM_MAX, NGRAM_MIN};

    #[test]
    fn candidate_tokenizer_uses_fixed_trigrams() {
        assert_eq!((NGRAM_MIN, NGRAM_MAX), (3, 3));
    }
}
