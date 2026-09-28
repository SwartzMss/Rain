use std::time::Instant;

use actix_web::web;

use crate::{
    AppState,
    error::AppError,
    log_expression::Expression,
    repositories::files::nearest_line_offset,
    search::{
        ContentSearchRequest, ContentSearchRow, ContentSearchScope, HARD_MAX_SEARCH_WINDOW,
        publication::{
            acquire_generation_lease_with_registry, artifact_relative_path,
            can_skip_visibility_snapshot,
        },
        search_tantivy_bundle_visible_with_lease_and_permit,
        search_tantivy_bundle_visible_with_lease_and_permit_and_context,
        search_tantivy_bundle_with_lease_and_permit,
        search_tantivy_bundle_with_lease_and_permit_and_context,
        visibility::snapshot_file_ids,
    },
    services::search_execution::{SearchExecutionContext, StopReason},
    services::temp_results::{CandidateScanPlan, SourceSearchPlan, merge_line_ranges},
};

pub(crate) use crate::services::temp_results::LineRange;

#[derive(Debug, Clone)]
pub(crate) struct IndexedSource {
    pub bundle_id: String,
    pub file_id: i64,
}

#[derive(Debug)]
pub(crate) struct PreviewSearchPlan {
    pub source_plans: Vec<SourceSearchPlan>,
    pub candidate_count: usize,
    pub query_elapsed_ms: u128,
    pub fallback_reasons: Vec<&'static str>,
}

impl PreviewSearchPlan {
    pub(crate) fn backend_label(&self) -> &'static str {
        let indexed = self
            .source_plans
            .iter()
            .filter(|plan| matches!(plan, SourceSearchPlan::Tantivy(_)))
            .count();
        match (indexed, self.source_plans.len()) {
            (0, _) => "raw_scan",
            (indexed, total) if indexed == total => "tantivy",
            _ => "mixed",
        }
    }
}

type PublicationRow = (String, String, i64, Option<i64>, Option<i64>, i64, i64);

pub(crate) async fn build_source_search_plans_with_context(
    state: &web::Data<AppState>,
    expression: &Expression,
    indexed_sources: &[Option<IndexedSource>],
    context: Option<&SearchExecutionContext>,
) -> Result<PreviewSearchPlan, AppError> {
    checkpoint(context)?;
    let started = Instant::now();
    let (term, fallback_reason) = match classify_expression(expression) {
        SearchPlanKind::IndexedTerm => {
            let Expression::Term(term) = expression else {
                return Err(AppError::Config(
                    "indexed expression classification lost its term".into(),
                ));
            };
            (Some(term.clone()), None)
        }
        SearchPlanKind::RawFallback(reason) => (None, Some(reason)),
    };
    let mut source_plans = Vec::with_capacity(indexed_sources.len());
    let mut candidate_count = 0_usize;
    let mut fallback_reasons = Vec::new();
    for indexed_source in indexed_sources {
        checkpoint(context)?;
        let plan = if let Some(reason) = fallback_reason {
            fallback_reasons.push(reason);
            SourceSearchPlan::Raw { reason }
        } else if let Some(indexed_source) = indexed_source {
            match build_indexed_source_plan(
                state,
                indexed_source,
                term.as_deref().expect("indexed term is present"),
                context,
            )
            .await
            {
                Ok((plan, count)) => {
                    candidate_count = candidate_count.saturating_add(count);
                    plan
                }
                Err(SearchPlanFailure::Fallback(reason)) => {
                    fallback_reasons.push(reason);
                    SourceSearchPlan::Raw { reason }
                }
                Err(SearchPlanFailure::Stopped(error)) => return Err(error),
            }
        } else {
            fallback_reasons.push("source_identity_missing");
            SourceSearchPlan::Raw {
                reason: "source_identity_missing",
            }
        };
        source_plans.push(plan);
    }
    Ok(PreviewSearchPlan {
        source_plans,
        candidate_count,
        query_elapsed_ms: started.elapsed().as_millis(),
        fallback_reasons,
    })
}

async fn build_indexed_source_plan(
    state: &web::Data<AppState>,
    source: &IndexedSource,
    term: &str,
    context: Option<&SearchExecutionContext>,
) -> Result<(SourceSearchPlan, usize), SearchPlanFailure> {
    checkpoint(context).map_err(SearchPlanFailure::Stopped)?;
    let publication: Option<PublicationRow> = sqlx::query_as(
        "SELECT backend, state, generation, schema_version, tokenizer_version, visibility_revision, compacted_revision FROM bundle_search_indexes WHERE bundle_id = ?",
    )
    .bind(&source.bundle_id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|_| SearchPlanFailure::Fallback("publication_lookup_failed"))?;
    let Some((
        backend,
        state_name,
        generation,
        schema_version,
        tokenizer_version,
        visibility_revision,
        compacted_revision,
    )) = publication
    else {
        return Err(SearchPlanFailure::Fallback("index_not_published"));
    };
    if backend != "tantivy" {
        return Err(SearchPlanFailure::Fallback("index_backend_not_tantivy"));
    }
    if !matches!(state_name.as_str(), "READY" | "NEEDS_REBUILD") {
        return Err(SearchPlanFailure::Fallback("index_not_ready"));
    }
    if schema_version != Some(crate::search::publication::TANTIVY_SCHEMA_VERSION)
        || tokenizer_version != Some(crate::search::publication::TANTIVY_TOKENIZER_VERSION)
    {
        return Err(SearchPlanFailure::Fallback("index_version_unsupported"));
    }
    if generation <= 0 {
        return Err(SearchPlanFailure::Fallback("index_generation_invalid"));
    }
    let artifact = artifact_relative_path(&source.bundle_id, generation)
        .map_err(|_| SearchPlanFailure::Fallback("index_artifact_invalid"))?;
    let permit = if let Some(context) = context {
        let cancellation = context.cancellation_token();
        let remaining = context.remaining();
        tokio::select! {
            permit = state.search.query_permits.clone().acquire_owned() => permit
                .map_err(|_| SearchPlanFailure::Fallback("query_admission_unavailable"))?,
            _ = cancellation.cancelled() => {
                return Err(SearchPlanFailure::Stopped(StopReason::Cancelled.into_error()));
            }
            _ = tokio::time::sleep(remaining) => {
                return Err(SearchPlanFailure::Stopped(StopReason::TimedOut.into_error()));
            }
        }
    } else {
        state
            .search
            .query_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| SearchPlanFailure::Fallback("query_admission_unavailable"))?
    };
    checkpoint(context).map_err(SearchPlanFailure::Stopped)?;
    let lease = acquire_generation_lease_with_registry(
        &state.search.generation_leases,
        &state.db.pool,
        &source.bundle_id,
        generation,
    )
    .await
    .map_err(|_| SearchPlanFailure::Fallback("generation_lease_unavailable"))?;
    let request = ContentSearchRequest {
        scope: ContentSearchScope::Bundle {
            bundle_id: source.bundle_id.clone(),
            timeline: None,
            file_id: Some(source.file_id),
        },
        query: term.to_owned(),
        path_like: None,
        from: 0,
        size: HARD_MAX_SEARCH_WINDOW as i64,
    };
    let result =
        if can_skip_visibility_snapshot(&state_name, visibility_revision, compacted_revision) {
            if let Some(context) = context {
                search_tantivy_bundle_with_lease_and_permit_and_context(
                    state.storage.data_root.join(artifact),
                    request,
                    source.bundle_id.clone(),
                    generation,
                    lease,
                    permit,
                    context.clone(),
                )
                .await
            } else {
                search_tantivy_bundle_with_lease_and_permit(
                    state.storage.data_root.join(artifact),
                    request,
                    source.bundle_id.clone(),
                    generation,
                    lease,
                    permit,
                )
                .await
            }
        } else {
            let visible_file_ids = snapshot_file_ids(&state.db.pool, &source.bundle_id)
                .await
                .map_err(|_| SearchPlanFailure::Fallback("visibility_snapshot_unavailable"))?;
            if let Some(context) = context {
                search_tantivy_bundle_visible_with_lease_and_permit_and_context(
                    state.storage.data_root.join(artifact),
                    request,
                    visible_file_ids,
                    source.bundle_id.clone(),
                    generation,
                    lease,
                    permit,
                    context.clone(),
                )
                .await
            } else {
                search_tantivy_bundle_visible_with_lease_and_permit(
                    state.storage.data_root.join(artifact),
                    request,
                    visible_file_ids,
                    source.bundle_id.clone(),
                    generation,
                    lease,
                    permit,
                )
                .await
            }
        };
    let result = match result {
        Ok(result) => result,
        Err(error) if is_stop_error(&error) => return Err(SearchPlanFailure::Stopped(error)),
        Err(_) => return Err(SearchPlanFailure::Fallback("tantivy_query_failed")),
    };
    checkpoint(context).map_err(SearchPlanFailure::Stopped)?;
    if result.total < 0 || result.total > HARD_MAX_SEARCH_WINDOW as i64 {
        return Err(SearchPlanFailure::Fallback("candidate_window_overflow"));
    }
    if result.rows.iter().any(|row| row.file_id != source.file_id) {
        return Err(SearchPlanFailure::Fallback("candidate_file_scope_mismatch"));
    }
    let ranges = candidate_ranges_from_rows(&result.rows)
        .map_err(|reason| SearchPlanFailure::Fallback(reason))?;
    if ranges.is_empty() {
        return Ok((
            SourceSearchPlan::Tantivy(CandidateScanPlan {
                ranges,
                seek_line: 0,
                seek_offset: 0,
            }),
            0,
        ));
    }
    let (seek_line, seek_offset) =
        nearest_line_offset(&state.db.pool, source.file_id, ranges[0].start)
            .await
            .map_err(|_| SearchPlanFailure::Fallback("line_offset_lookup_failed"))?;
    let seek_offset = u64::try_from(seek_offset)
        .map_err(|_| SearchPlanFailure::Fallback("line_offset_invalid"))?;
    Ok((
        SourceSearchPlan::Tantivy(CandidateScanPlan {
            ranges,
            seek_line,
            seek_offset,
        }),
        result.rows.len(),
    ))
}

enum SearchPlanFailure {
    Fallback(&'static str),
    Stopped(AppError),
}

fn checkpoint(context: Option<&SearchExecutionContext>) -> Result<(), AppError> {
    if let Some(context) = context {
        context.checkpoint().map_err(StopReason::into_error)?;
    }
    Ok(())
}

fn is_stop_error(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Api {
            code: "SEARCH_CANCELLED",
            ..
        } | AppError::PublicApi {
            code: "TEMP_RESULT_SCAN_TIMEOUT",
            ..
        }
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchPlanKind {
    IndexedTerm,
    RawFallback(&'static str),
}

pub(crate) fn classify_expression(expression: &Expression) -> SearchPlanKind {
    let Expression::Term(term) = expression else {
        return SearchPlanKind::RawFallback("expression_not_a_term");
    };
    if term.chars().count() < 3 {
        return SearchPlanKind::RawFallback("term_too_short");
    }
    if term.chars().next().is_some_and(char::is_whitespace)
        || term.chars().next_back().is_some_and(char::is_whitespace)
    {
        return SearchPlanKind::RawFallback("term_has_edge_whitespace");
    }
    if term.contains('\0') {
        return SearchPlanKind::RawFallback("term_contains_nul");
    }
    if !term.is_ascii() {
        return SearchPlanKind::RawFallback("term_not_ascii");
    }
    // The raw matcher uses full Unicode case folding while Tantivy's
    // LowerCaser only performs lowercasing. Compatibility characters can
    // fold into ASCII variants or sequences that the index cannot produce
    // as a candidate (for example, U+017F -> "s" and U+FB03 -> "ffi").
    if ["k", "s", "ff", "fi", "fl"]
        .iter()
        .any(|fragment| term.contains(fragment))
    {
        return SearchPlanKind::RawFallback("term_has_unicode_casefold_variant");
    }
    SearchPlanKind::IndexedTerm
}

pub(crate) fn candidate_ranges_from_rows(
    rows: &[ContentSearchRow],
) -> Result<Vec<LineRange>, &'static str> {
    let mut ranges = Vec::with_capacity(rows.len());
    for row in rows {
        let (Some(start), Some(end)) = (row.offset, row.line_end) else {
            return Err("candidate_line_bounds_missing");
        };
        if start < 0 || end < start {
            return Err("candidate_line_bounds_invalid");
        }
        ranges.push(LineRange { start, end });
    }
    Ok(merge_line_ranges(ranges))
}

#[cfg(test)]
mod tests {
    use super::{LineRange, SearchPlanKind, candidate_ranges_from_rows, classify_expression};
    use crate::log_expression::parse;
    use crate::search::ContentSearchRow;
    use crate::services::temp_results::{CandidateScanPlan, SourceSearchPlan};

    #[test]
    fn only_safe_ascii_terms_use_tantivy_candidates() {
        assert_eq!(
            classify_expression(&parse("ERROR").unwrap()),
            SearchPlanKind::IndexedTerm
        );
        assert_eq!(
            classify_expression(&parse(r#""ERROR log""#).unwrap()),
            SearchPlanKind::IndexedTerm
        );
        assert_eq!(
            classify_expression(&parse("RARE_VALUE").unwrap()),
            SearchPlanKind::IndexedTerm
        );
        assert_eq!(
            classify_expression(&parse("ab").unwrap()),
            SearchPlanKind::RawFallback("term_too_short")
        );
        assert_eq!(
            classify_expression(&parse("ERROR AND timeout").unwrap()),
            SearchPlanKind::RawFallback("expression_not_a_term")
        );
        assert_eq!(
            classify_expression(&parse("错误标记").unwrap()),
            SearchPlanKind::RawFallback("term_not_ascii")
        );
        assert_eq!(
            classify_expression(&parse("office").unwrap()),
            SearchPlanKind::RawFallback("term_has_unicode_casefold_variant")
        );
        assert_eq!(
            classify_expression(&parse("sab").unwrap()),
            SearchPlanKind::RawFallback("term_has_unicode_casefold_variant")
        );
    }

    #[test]
    fn candidate_rows_become_file_scoped_line_ranges() {
        let rows = vec![
            ContentSearchRow {
                file_id: 42,
                path: "/app.log".into(),
                bundle_hash: None,
                timeline: None,
                offset: Some(10),
                line_end: Some(12),
                chunk_index: Some(2),
                content: "marker".into(),
            },
            ContentSearchRow {
                file_id: 42,
                path: "/app.log".into(),
                bundle_hash: None,
                timeline: None,
                offset: Some(13),
                line_end: Some(20),
                chunk_index: Some(3),
                content: "marker".into(),
            },
        ];

        assert_eq!(
            candidate_ranges_from_rows(&rows),
            Ok(vec![LineRange { start: 10, end: 20 }])
        );
    }

    #[test]
    fn malformed_candidate_rows_force_raw_fallback() {
        let rows = vec![ContentSearchRow {
            file_id: 42,
            path: "/app.log".into(),
            bundle_hash: None,
            timeline: None,
            offset: None,
            line_end: Some(12),
            chunk_index: Some(2),
            content: "marker".into(),
        }];

        assert_eq!(
            candidate_ranges_from_rows(&rows),
            Err("candidate_line_bounds_missing")
        );
    }

    #[test]
    fn preview_plan_labels_mixed_backends_and_preserves_fallback_reason() {
        let plan = super::PreviewSearchPlan {
            source_plans: vec![
                SourceSearchPlan::Tantivy(CandidateScanPlan {
                    ranges: vec![LineRange { start: 0, end: 0 }],
                    seek_line: 0,
                    seek_offset: 0,
                }),
                SourceSearchPlan::Raw {
                    reason: "source_identity_missing",
                },
            ],
            candidate_count: 1,
            query_elapsed_ms: 0,
            fallback_reasons: vec!["source_identity_missing"],
        };

        assert_eq!(plan.backend_label(), "mixed");
        assert_eq!(plan.fallback_reasons, vec!["source_identity_missing"]);
    }
}
