use crate::{Album, Artist, Track};

pub const SEARCH_RESULT_LIMIT: usize = 20;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRequest {
    query: String,
    limit: usize,
}

impl SearchRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: bounded_query(query.into()),
            limit: SEARCH_RESULT_LIMIT,
        }
    }

    pub fn with_limit(query: impl Into<String>, limit: usize) -> Self {
        Self {
            query: bounded_query(query.into()),
            limit: limit.clamp(1, 100),
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub const fn limit(&self) -> usize {
        self.limit
    }
}

fn bounded_query(query: String) -> String {
    query.chars().take(256).collect()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchResults {
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
}

impl SearchResults {
    pub fn is_empty(&self) -> bool {
        self.artists.is_empty() && self.albums.is_empty() && self.tracks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_request_bounds_provider_and_projection_work() {
        let request = SearchRequest::with_limit("a".repeat(300), usize::MAX);
        assert_eq!(request.query().chars().count(), 256);
        assert_eq!(request.limit(), 100);
    }
}
