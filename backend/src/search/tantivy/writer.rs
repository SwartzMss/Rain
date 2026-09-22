use std::path::Path;

use tantivy::{Index, IndexSettings, IndexWriter, TantivyDocument, directory::MmapDirectory};

use crate::error::AppError;

use super::{schema::BundleSchema, tokenizer};

#[derive(Debug, Clone)]
pub struct IndexedChunk {
    pub file_id: i64,
    pub chunk_index: i64,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
    pub event_time_start_ms: Option<i64>,
    pub event_time_end_ms: Option<i64>,
    pub timeline: Option<String>,
    pub content: String,
    pub path: String,
}

pub struct BundleIndexWriter {
    pub(crate) index: Index,
    pub(crate) fields: BundleSchema,
    writer: IndexWriter,
}

impl BundleIndexWriter {
    pub fn create(path: impl AsRef<Path>, heap_size_bytes: usize) -> Result<Self, AppError> {
        std::fs::create_dir_all(path.as_ref()).map_err(AppError::Io)?;
        let fields = super::schema::build_schema();
        let directory = MmapDirectory::open(path.as_ref())
            .map_err(|error| AppError::Config(format!("open Tantivy directory: {error}")))?;
        let index = Index::create(directory, fields.schema.clone(), IndexSettings::default())
            .map_err(|error| AppError::Config(format!("create Tantivy index: {error}")))?;
        index
            .tokenizers()
            .register(TOKENIZER_NAME, tokenizer::analyzer());
        let writer = index
            .writer(heap_size_bytes)
            .map_err(|error| AppError::Config(format!("create Tantivy writer: {error}")))?;
        Ok(Self {
            index,
            fields,
            writer,
        })
    }

    pub fn add_chunk(&mut self, chunk: &IndexedChunk) -> Result<(), AppError> {
        let fields = &self.fields;
        let mut document = TantivyDocument::default();
        document.add_text(fields.content, &chunk.content);
        document.add_u64(fields.file_id, chunk.file_id.max(0) as u64);
        document.add_u64(fields.chunk_index, chunk.chunk_index.max(0) as u64);
        if let Some(value) = chunk.line_start {
            document.add_i64(fields.line_start, value);
        }
        if let Some(value) = chunk.line_end {
            document.add_i64(fields.line_end, value);
        }
        if let Some(value) = chunk.event_time_start_ms {
            document.add_i64(fields.event_time_start, value);
        }
        if let Some(value) = chunk.event_time_end_ms {
            document.add_i64(fields.event_time_end, value);
        }
        document.add_bool(
            fields.event_time_indexed,
            chunk.event_time_start_ms.is_some() && chunk.event_time_end_ms.is_some(),
        );
        if let Some(value) = &chunk.timeline {
            document.add_text(fields.timeline, value);
        }
        document.add_text(fields.path, &chunk.path);
        self.writer
            .add_document(document)
            .map_err(|error| AppError::Config(format!("add Tantivy document: {error}")))?;
        Ok(())
    }

    pub fn commit(mut self) -> Result<CommittedBundleIndex, AppError> {
        self.writer
            .commit()
            .map_err(|error| AppError::Config(format!("commit Tantivy index: {error}")))?;
        self.writer
            .wait_merging_threads()
            .map_err(|error| AppError::Config(format!("merge Tantivy index: {error}")))?;
        let reader = self
            .index
            .reader()
            .map_err(|error| AppError::Config(format!("open Tantivy reader: {error}")))?;
        let document_count = reader.searcher().num_docs();
        Ok(CommittedBundleIndex {
            index: self.index,
            fields: self.fields,
            document_count,
        })
    }
}

/// Reopen a committed bundle index and verify that Tantivy can read its
/// metadata. Publication calls this after the staging directory is moved into
/// its immutable generation path.
pub fn open_committed(path: impl AsRef<Path>) -> Result<CommittedBundleIndex, AppError> {
    let fields = super::schema::build_schema();
    let directory = MmapDirectory::open(path.as_ref())
        .map_err(|error| AppError::Config(format!("open Tantivy directory: {error}")))?;
    let index = Index::open(directory)
        .map_err(|error| AppError::Config(format!("open committed Tantivy index: {error}")))?;
    index
        .tokenizers()
        .register(TOKENIZER_NAME, tokenizer::analyzer());
    let reader = index
        .reader()
        .map_err(|error| AppError::Config(format!("open committed Tantivy reader: {error}")))?;
    Ok(CommittedBundleIndex {
        index,
        fields,
        document_count: reader.searcher().num_docs(),
    })
}

pub struct CommittedBundleIndex {
    pub(crate) index: Index,
    pub(crate) fields: BundleSchema,
    pub document_count: u64,
}

const TOKENIZER_NAME: &str = super::tokenizer::TOKENIZER_NAME;
