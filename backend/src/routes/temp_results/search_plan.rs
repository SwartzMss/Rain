use std::{
    collections::{BTreeMap, HashMap},
    time::Instant,
};

use actix_web::web;
use futures_util::StreamExt;

use crate::{
    AppState,
    error::AppError,
    log_expression::Expression,
    repositories::files::nearest_line_offsets,
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
    pub candidate_strategy: &'static str,
    pub candidate_term_count: usize,
    pub boolean_node_count: usize,
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
    let boolean_node_count = expression_node_count(expression);
    let (terms, fallback_reason) = match candidate_plan(expression) {
        Some(CandidatePlan::Terms(terms)) => (terms, None),
        None => (Vec::new(), Some(expression_fallback_reason(expression))),
    };
    let candidate_strategy = match (fallback_reason, terms.len()) {
        (Some(_), _) => "full_scan",
        (None, 1) => "anchor_term",
        (None, _) => "union",
    };
    let mut pending: Vec<Option<SourcePlanResult>> =
        (0..indexed_sources.len()).map(|_| None).collect();

    if let Some(reason) = fallback_reason {
        for slot in pending.iter_mut() {
            checkpoint(context)?;
            *slot = Some(Err(reason));
        }
    } else {
        let mut bundles: BTreeMap<String, Vec<(usize, IndexedSource)>> = BTreeMap::new();
        for (index, indexed_source) in indexed_sources.iter().enumerate() {
            checkpoint(context)?;
            if let Some(indexed_source) = indexed_source {
                bundles
                    .entry(indexed_source.bundle_id.clone())
                    .or_default()
                    .push((index, indexed_source.clone()));
            } else {
                pending[index] = Some(Err("source_identity_missing"));
            }
        }
        let bundle_count = bundles.len();
        let mut bundle_plans = futures_util::stream::iter(bundles.into_values().map(|sources| {
            let state = state.clone();
            let terms = terms.clone();
            async move { build_indexed_bundle_plans(&state, &sources, &terms, context).await }
        }))
        .buffer_unordered(crate::search::parallel::issue_search_parallelism(
            bundle_count,
        ));
        while let Some(plans) = bundle_plans.next().await {
            checkpoint(context)?;
            for (index, plan) in plans? {
                pending[index] = Some(plan);
            }
        }
    }

    let mut source_plans = Vec::with_capacity(indexed_sources.len());
    let mut candidate_count = 0_usize;
    let mut fallback_reasons = Vec::new();
    for plan in pending {
        let plan = plan.ok_or_else(|| {
            AppError::Config("temporary result source search plan was not populated".into())
        })?;
        match plan {
            Ok((plan, count)) => {
                candidate_count = candidate_count.saturating_add(count);
                source_plans.push(plan);
            }
            Err(reason) => {
                fallback_reasons.push(reason);
                source_plans.push(SourceSearchPlan::Raw { reason });
            }
        }
    }
    Ok(PreviewSearchPlan {
        source_plans,
        candidate_count,
        query_elapsed_ms: started.elapsed().as_millis(),
        fallback_reasons,
        candidate_strategy,
        candidate_term_count: terms.len(),
        boolean_node_count,
    })
}

type SourcePlanResult = Result<(SourceSearchPlan, usize), &'static str>;

async fn acquire_query_permit(
    state: &web::Data<AppState>,
    context: Option<&SearchExecutionContext>,
) -> Result<tokio::sync::OwnedSemaphorePermit, AppError> {
    if let Some(context) = context {
        let cancellation = context.cancellation_token();
        let remaining = context.remaining();
        tokio::select! {
            permit = state.search.query_permits.clone().acquire_owned() => permit
                .map_err(|_| AppError::Config("query admission unavailable".into())),
            _ = cancellation.cancelled() => Err(StopReason::Cancelled.into_error()),
            _ = tokio::time::sleep(remaining) => Err(StopReason::TimedOut.into_error()),
        }
    } else {
        state
            .search
            .query_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AppError::Config("query admission unavailable".into()))
    }
}

fn fallback_bundle_plans(
    sources: &[(usize, IndexedSource)],
    reason: &'static str,
) -> Vec<(usize, SourcePlanResult)> {
    sources
        .iter()
        .map(|(index, _)| (*index, Err(reason)))
        .collect()
}

async fn build_indexed_bundle_plans(
    state: &web::Data<AppState>,
    sources: &[(usize, IndexedSource)],
    terms: &[String],
    context: Option<&SearchExecutionContext>,
) -> Result<Vec<(usize, SourcePlanResult)>, AppError> {
    let Some((_, first_source)) = sources.first() else {
        return Ok(Vec::new());
    };
    checkpoint(context)?;
    let publication: Option<PublicationRow> = match sqlx::query_as(
        "SELECT backend, state, generation, schema_version, tokenizer_version, visibility_revision, compacted_revision FROM bundle_search_indexes WHERE bundle_id = ?",
    )
    .bind(&first_source.bundle_id)
    .fetch_optional(&state.db.pool)
    .await
    {
        Ok(publication) => publication,
        Err(_) => return Ok(fallback_bundle_plans(sources, "publication_lookup_failed")),
    };
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
        return Ok(fallback_bundle_plans(sources, "index_not_published"));
    };
    if backend != "tantivy" {
        return Ok(fallback_bundle_plans(sources, "index_backend_not_tantivy"));
    }
    if !matches!(state_name.as_str(), "READY" | "NEEDS_REBUILD") {
        return Ok(fallback_bundle_plans(sources, "index_not_ready"));
    }
    if schema_version != Some(crate::search::publication::TANTIVY_SCHEMA_VERSION)
        || tokenizer_version != Some(crate::search::publication::TANTIVY_TOKENIZER_VERSION)
    {
        return Ok(fallback_bundle_plans(sources, "index_version_unsupported"));
    }
    if generation <= 0 {
        return Ok(fallback_bundle_plans(sources, "index_generation_invalid"));
    }
    let artifact = match artifact_relative_path(&first_source.bundle_id, generation) {
        Ok(artifact) => artifact,
        Err(_) => return Ok(fallback_bundle_plans(sources, "index_artifact_invalid")),
    };
    let visible_file_ids =
        if can_skip_visibility_snapshot(&state_name, visibility_revision, compacted_revision) {
            None
        } else {
            match snapshot_file_ids(&state.db.pool, &first_source.bundle_id).await {
                Ok(visible_file_ids) => Some(visible_file_ids),
                Err(_) => {
                    return Ok(fallback_bundle_plans(
                        sources,
                        "visibility_snapshot_unavailable",
                    ));
                }
            }
        };
    // Every positive branch is queried independently. Each query owns its
    // permit and generation lease, while the rows are unioned before they are
    // converted into file-scoped ranges. If any branch is incomplete, the
    // whole Bundle falls back so the union can never miss a match.
    let mut candidate_rows = Vec::new();
    let mut candidate_total = 0_i64;
    for term in terms {
        checkpoint(context)?;
        let permit = match acquire_query_permit(state, context).await {
            Ok(permit) => permit,
            Err(error) if is_stop_error(&error) => return Err(error),
            Err(_) => {
                return Ok(fallback_bundle_plans(
                    sources,
                    "query_admission_unavailable",
                ));
            }
        };
        let lease = match acquire_generation_lease_with_registry(
            &state.search.generation_leases,
            &state.db.pool,
            &first_source.bundle_id,
            generation,
        )
        .await
        {
            Ok(lease) => lease,
            Err(_) => {
                return Ok(fallback_bundle_plans(
                    sources,
                    "generation_lease_unavailable",
                ));
            }
        };
        let request = ContentSearchRequest {
            scope: ContentSearchScope::Bundle {
                bundle_id: first_source.bundle_id.clone(),
                timeline: None,
                file_id: (sources.len() == 1).then_some(first_source.file_id),
            },
            query: term.clone(),
            path_like: None,
            file_ids: Some(sources.iter().map(|(_, source)| source.file_id).collect()),
            from: 0,
            size: HARD_MAX_SEARCH_WINDOW as i64,
            include_content: false,
        };
        let result = if let Some(visible_file_ids) = visible_file_ids.clone() {
            if let Some(context) = context {
                search_tantivy_bundle_visible_with_lease_and_permit_and_context(
                    state.storage.data_root.join(&artifact),
                    request,
                    visible_file_ids,
                    first_source.bundle_id.clone(),
                    generation,
                    lease,
                    permit,
                    context.clone(),
                )
                .await
            } else {
                search_tantivy_bundle_visible_with_lease_and_permit(
                    state.storage.data_root.join(&artifact),
                    request,
                    visible_file_ids,
                    first_source.bundle_id.clone(),
                    generation,
                    lease,
                    permit,
                )
                .await
            }
        } else if let Some(context) = context {
            search_tantivy_bundle_with_lease_and_permit_and_context(
                state.storage.data_root.join(&artifact),
                request,
                first_source.bundle_id.clone(),
                generation,
                lease,
                permit,
                context.clone(),
            )
            .await
        } else {
            search_tantivy_bundle_with_lease_and_permit(
                state.storage.data_root.join(&artifact),
                request,
                first_source.bundle_id.clone(),
                generation,
                lease,
                permit,
            )
            .await
        };
        let result = match result {
            Ok(result) => result,
            Err(error) if is_stop_error(&error) => return Err(error),
            Err(_) => return Ok(fallback_bundle_plans(sources, "tantivy_query_failed")),
        };
        checkpoint(context)?;
        if result.total < 0
            || result.total > HARD_MAX_SEARCH_WINDOW as i64
            || result.truncated
            || usize::try_from(result.total)
                .ok()
                .is_none_or(|total| total > result.rows.len())
        {
            return Ok(fallback_bundle_plans(sources, "candidate_window_overflow"));
        }
        candidate_total = candidate_total
            .checked_add(result.total)
            .ok_or_else(|| AppError::Config("candidate total overflow".into()))?;
        if candidate_total > HARD_MAX_SEARCH_WINDOW as i64 {
            return Ok(fallback_bundle_plans(sources, "candidate_window_overflow"));
        }
        candidate_rows.extend(result.rows);
    }

    let mut rows_by_file: HashMap<i64, Vec<ContentSearchRow>> = HashMap::new();
    for row in candidate_rows {
        rows_by_file.entry(row.file_id).or_default().push(row);
    }
    let offset_requests = sources
        .iter()
        .filter_map(|(_, source)| {
            rows_by_file
                .get(&source.file_id)
                .and_then(|rows| candidate_ranges_from_rows(rows).ok())
                .and_then(|ranges| ranges.first().map(|range| (source.file_id, range.start)))
        })
        .collect::<Vec<_>>();
    let offsets = match nearest_line_offsets(&state.db.pool, &offset_requests).await {
        Ok(offsets) => offsets,
        Err(_) => return Ok(fallback_bundle_plans(sources, "line_offset_lookup_failed")),
    };
    let mut plans = Vec::with_capacity(sources.len());
    for (index, source) in sources {
        checkpoint(context)?;
        let rows = rows_by_file.remove(&source.file_id).unwrap_or_default();
        let plan =
            match build_source_plan_from_rows(rows, offsets.get(&source.file_id).copied(), context)
            {
                Ok(plan) => Ok(plan),
                Err(SearchPlanFailure::Fallback(reason)) => Err(reason),
                Err(SearchPlanFailure::Stopped(error)) => return Err(error),
            };
        plans.push((*index, plan));
    }
    Ok(plans)
}

fn build_source_plan_from_rows(
    rows: Vec<ContentSearchRow>,
    seek: Option<(i64, i64)>,
    context: Option<&SearchExecutionContext>,
) -> Result<(SourceSearchPlan, usize), SearchPlanFailure> {
    checkpoint(context).map_err(SearchPlanFailure::Stopped)?;
    let ranges = candidate_ranges_from_rows(&rows).map_err(SearchPlanFailure::Fallback)?;
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
        seek.ok_or(SearchPlanFailure::Fallback("line_offset_lookup_failed"))?;
    let seek_offset = u64::try_from(seek_offset)
        .map_err(|_| SearchPlanFailure::Fallback("line_offset_invalid"))?;
    checkpoint(context).map_err(SearchPlanFailure::Stopped)?;
    Ok((
        SourceSearchPlan::Tantivy(CandidateScanPlan {
            ranges,
            seek_line,
            seek_offset,
        }),
        rows.len(),
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum CandidatePlan {
    Terms(Vec<String>),
}

fn candidate_plan(expression: &Expression) -> Option<CandidatePlan> {
    match expression {
        Expression::Term(term) => match classify_expression(expression) {
            SearchPlanKind::IndexedTerm => Some(CandidatePlan::Terms(vec![term.clone()])),
            SearchPlanKind::RawFallback(_) => None,
        },
        Expression::Not(_) => None,
        Expression::And(left, right) => {
            let left = candidate_plan(left);
            let right = candidate_plan(right);
            match (left, right) {
                (Some(left), Some(right)) => Some(prefer_candidate(left, right)),
                (Some(plan), None) | (None, Some(plan)) => Some(plan),
                (None, None) => None,
            }
        }
        Expression::Or(left, right) => match (candidate_plan(left), candidate_plan(right)) {
            (Some(CandidatePlan::Terms(left)), Some(CandidatePlan::Terms(right))) => {
                let mut terms = left;
                for term in right {
                    if !terms.contains(&term) {
                        terms.push(term);
                    }
                }
                Some(CandidatePlan::Terms(terms))
            }
            _ => None,
        },
    }
}

fn prefer_candidate(left: CandidatePlan, right: CandidatePlan) -> CandidatePlan {
    let left_len = match &left {
        CandidatePlan::Terms(terms) => terms.len(),
    };
    let right_len = match &right {
        CandidatePlan::Terms(terms) => terms.len(),
    };
    if left_len <= right_len { left } else { right }
}

fn expression_fallback_reason(expression: &Expression) -> &'static str {
    match expression {
        Expression::Term(_) => match classify_expression(expression) {
            SearchPlanKind::RawFallback(reason) => reason,
            SearchPlanKind::IndexedTerm => "expression_not_indexable",
        },
        _ => "expression_not_indexable",
    }
}

fn expression_node_count(expression: &Expression) -> usize {
    match expression {
        Expression::Term(_) => 1,
        Expression::Not(child) => 1 + expression_node_count(child),
        Expression::And(left, right) | Expression::Or(left, right) => {
            1 + expression_node_count(left) + expression_node_count(right)
        }
    }
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
    use super::{
        CandidatePlan, LineRange, SearchPlanKind, candidate_plan, candidate_ranges_from_rows,
        classify_expression,
    };
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
            candidate_plan(&parse("ERROR AND timeout").unwrap()),
            Some(CandidatePlan::Terms(vec!["error".into()]))
        );
        assert_eq!(
            candidate_plan(&parse("ERROR AND NOT timeout").unwrap()),
            Some(CandidatePlan::Terms(vec!["error".into()]))
        );
        assert_eq!(
            candidate_plan(&parse("ERROR AND (timeout OR latency)").unwrap()),
            Some(CandidatePlan::Terms(vec!["error".into()]))
        );
        let parenthesized = parse("(ERROR OR timeout) AND latency").unwrap();
        assert_eq!(
            candidate_plan(&parenthesized),
            Some(CandidatePlan::Terms(vec!["latency".into()]))
        );
        assert_eq!(
            candidate_plan(&parse("ERROR OR timeout").unwrap()),
            Some(CandidatePlan::Terms(vec!["error".into(), "timeout".into()]))
        );
        assert_eq!(
            candidate_plan(&parse("ERROR OR NOT timeout").unwrap()),
            None
        );
        assert_eq!(candidate_plan(&parse("NOT ERROR").unwrap()), None);
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
                tantivy_doc_address: None,
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
                tantivy_doc_address: None,
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
            tantivy_doc_address: None,
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
            candidate_strategy: "anchor_term",
            candidate_term_count: 1,
            boolean_node_count: 1,
        };

        assert_eq!(plan.backend_label(), "mixed");
        assert_eq!(plan.fallback_reasons, vec!["source_identity_missing"]);
    }
}
