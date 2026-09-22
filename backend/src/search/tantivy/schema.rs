use tantivy::schema::{Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions};

use super::tokenizer::TOKENIZER_NAME;

pub const INDEX_FORMAT_VERSION: u32 = 1;

#[derive(Clone)]
pub struct BundleSchema {
    pub schema: Schema,
    pub content: Field,
    pub file_id: Field,
    pub chunk_index: Field,
    pub line_start: Field,
    pub line_end: Field,
    pub event_time_start: Field,
    pub event_time_end: Field,
    pub event_time_indexed: Field,
    pub path: Field,
}

pub fn build_schema() -> BundleSchema {
    let mut builder = Schema::builder();
    let content = builder.add_text_field(
        "content",
        TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(TOKENIZER_NAME)
                    .set_index_option(IndexRecordOption::Basic),
            )
            .set_stored(),
    );
    let path = builder.add_text_field("path", TextOptions::default().set_stored());
    let file_id = builder.add_u64_field(
        "file_id",
        tantivy::schema::NumericOptions::default()
            .set_stored()
            .set_indexed(),
    );
    let chunk_index = builder.add_u64_field(
        "chunk_index",
        tantivy::schema::NumericOptions::default().set_stored(),
    );
    let line_start = builder.add_i64_field(
        "line_start",
        tantivy::schema::NumericOptions::default().set_stored(),
    );
    let line_end = builder.add_i64_field(
        "line_end",
        tantivy::schema::NumericOptions::default().set_stored(),
    );
    let event_time_start = builder.add_i64_field(
        "event_time_start",
        tantivy::schema::NumericOptions::default().set_stored(),
    );
    let event_time_end = builder.add_i64_field(
        "event_time_end",
        tantivy::schema::NumericOptions::default().set_stored(),
    );
    let event_time_indexed = builder.add_bool_field(
        "event_time_indexed",
        tantivy::schema::NumericOptions::default().set_stored(),
    );
    BundleSchema {
        schema: builder.build(),
        content,
        file_id,
        chunk_index,
        line_start,
        line_end,
        event_time_start,
        event_time_end,
        event_time_indexed,
        path,
    }
}
