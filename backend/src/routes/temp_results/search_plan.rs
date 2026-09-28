use crate::log_expression::Expression;

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
    SearchPlanKind::IndexedTerm
}

#[cfg(test)]
mod tests {
    use super::{SearchPlanKind, classify_expression};
    use crate::log_expression::parse;

    #[test]
    fn only_safe_ascii_terms_use_tantivy_candidates() {
        assert_eq!(
            classify_expression(&parse("ERROR").unwrap()),
            SearchPlanKind::IndexedTerm
        );
        assert_eq!(
            classify_expression(&parse(r#""ERROR smoke""#).unwrap()),
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
    }
}
