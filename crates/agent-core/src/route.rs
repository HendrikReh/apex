use crate::types::{QueryClass, RetrievalProfileId, RouteDecision, RoutePath};

pub fn route_query(query: &str) -> RouteDecision {
    let lower = query.to_ascii_lowercase();

    let has_compare =
        lower.contains("compare ") || lower.contains("tradeoff") || lower.contains("vs ");
    let has_how_to =
        lower.contains("how do i") || lower.contains("runbook") || lower.contains("steps");
    let has_ambiguity = lower.contains("which ") || lower.contains("difference between");
    let has_exploratory = lower.contains("find ")
        || lower.contains("show me")
        || lower.contains("search for")
        || lower.contains("overview of")
        || lower.contains("documents about")
        || lower.contains("docs about");
    let has_time = lower.contains("today") || lower.contains("latest") || lower.contains("current");

    let (selected_path, query_class, retrieval_profile, needs_multi_hop) = if has_compare {
        (
            RoutePath::AgenticSearch,
            QueryClass::MultiHopResearch,
            RetrievalProfileId::BroadThenExpand,
            true,
        )
    } else if has_how_to {
        (RoutePath::AgenticSearch, QueryClass::Procedural, RetrievalProfileId::LexicalFirst, false)
    } else if has_ambiguity {
        (
            RoutePath::AgenticSearch,
            QueryClass::AmbiguityDisambiguation,
            RetrievalProfileId::LexicalFirst,
            false,
        )
    } else if has_exploratory {
        (
            RoutePath::AgenticSearch,
            QueryClass::ExploratorySearch,
            RetrievalProfileId::BroadThenExpand,
            false,
        )
    } else {
        (RoutePath::SinglePassRag, QueryClass::SimpleFact, RetrievalProfileId::SimpleHybrid, false)
    };

    let mut reasons = Vec::new();
    if has_compare {
        reasons.push("compare".to_string());
    }
    if has_how_to {
        reasons.push("procedural".to_string());
    }
    if has_ambiguity {
        reasons.push("ambiguity".to_string());
    }
    if has_exploratory {
        reasons.push("exploratory".to_string());
    }
    if reasons.is_empty() {
        reasons.push("default".to_string());
    }
    if has_time {
        reasons.push("time_sensitive".to_string());
    }

    RouteDecision {
        selected_path,
        query_class,
        retrieval_profile,
        ambiguity: has_ambiguity,
        needs_multi_hop,
        needs_high_evidence: has_compare || has_how_to || has_exploratory,
        time_sensitive: has_time,
        normalized_filters: Vec::new(),
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_fact_routes_to_single_pass_rag() {
        let decision = route_query("What is Rust?");
        assert_eq!(decision.selected_path, RoutePath::SinglePassRag);
        assert_eq!(decision.query_class, QueryClass::SimpleFact);
        assert_eq!(decision.retrieval_profile, RetrievalProfileId::SimpleHybrid);
    }

    #[test]
    fn multi_hop_query_routes_to_agentic_search() {
        let decision = route_query("Compare Rust and Python tradeoffs for async services");
        assert_eq!(decision.selected_path, RoutePath::AgenticSearch);
        assert_eq!(decision.query_class, QueryClass::MultiHopResearch);
        assert_eq!(decision.retrieval_profile, RetrievalProfileId::BroadThenExpand);
        assert!(decision.needs_multi_hop);
    }

    #[test]
    fn procedural_query_prefers_lexical_first() {
        let decision = route_query("How do I rotate API keys in the auth runbook?");
        assert_eq!(decision.selected_path, RoutePath::AgenticSearch);
        assert_eq!(decision.query_class, QueryClass::Procedural);
        assert_eq!(decision.retrieval_profile, RetrievalProfileId::LexicalFirst);
    }

    #[test]
    fn exploratory_query_routes_to_agentic_search() {
        let decision = route_query("Find documents about rollback timing for release incidents");
        assert_eq!(decision.selected_path, RoutePath::AgenticSearch);
        assert_eq!(decision.query_class, QueryClass::ExploratorySearch);
        assert_eq!(decision.retrieval_profile, RetrievalProfileId::BroadThenExpand);
    }
}
