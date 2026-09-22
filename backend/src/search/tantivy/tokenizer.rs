use tantivy::tokenizer::{LowerCaser, NgramTokenizer, TextAnalyzer};

pub const TOKENIZER_NAME: &str = "rain_ngram_v1";
pub const NGRAM_MIN: usize = 2;
pub const NGRAM_MAX: usize = 20;

pub fn analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(
        NgramTokenizer::all_ngrams(NGRAM_MIN, NGRAM_MAX)
            .expect("valid Tantivy n-gram tokenizer bounds"),
    )
    .filter(LowerCaser)
    .build()
}
