use crate::domain::{
    SmartPlaylistBuiltin, SmartPlaylistDefinition, SmartPlaylistMatchMode, SmartPlaylistRule,
    SmartPlaylistRuleField, SmartPlaylistRuleGroup, SmartPlaylistRuleNode,
    SmartPlaylistRuleOperator, SmartPlaylistRuleValue, SmartPlaylistSortField,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SmartPlaylistRuleValueKind {
    None,
    Text,
    Number,
    NumberRange,
    Date,
    DateRange,
    Bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SmartPlaylistRuleOp {
    pub operator: SmartPlaylistRuleOperator,
    pub value_kind: SmartPlaylistRuleValueKind,
}

const RULE_FIELDS: [SmartPlaylistRuleField; 13] = [
    SmartPlaylistRuleField::Title,
    SmartPlaylistRuleField::Artist,
    SmartPlaylistRuleField::Album,
    SmartPlaylistRuleField::Comment,
    SmartPlaylistRuleField::Genre,
    SmartPlaylistRuleField::Rating,
    SmartPlaylistRuleField::Year,
    SmartPlaylistRuleField::Favorite,
    SmartPlaylistRuleField::Played,
    SmartPlaylistRuleField::PlayCount,
    SmartPlaylistRuleField::SkipCount,
    SmartPlaylistRuleField::LastPlayed,
    SmartPlaylistRuleField::DateAdded,
];

const SORT_FIELDS: [SmartPlaylistSortField; 10] = [
    SmartPlaylistSortField::Title,
    SmartPlaylistSortField::Artist,
    SmartPlaylistSortField::Album,
    SmartPlaylistSortField::Year,
    SmartPlaylistSortField::DateAdded,
    SmartPlaylistSortField::LastPlayed,
    SmartPlaylistSortField::PlayCount,
    SmartPlaylistSortField::SkipCount,
    SmartPlaylistSortField::Rating,
    SmartPlaylistSortField::Duration,
];

const TEXT_OPS: [SmartPlaylistRuleOp; 6] = [
    op(
        SmartPlaylistRuleOperator::Contains,
        SmartPlaylistRuleValueKind::Text,
    ),
    op(
        SmartPlaylistRuleOperator::Equals,
        SmartPlaylistRuleValueKind::Text,
    ),
    op(
        SmartPlaylistRuleOperator::NotContains,
        SmartPlaylistRuleValueKind::Text,
    ),
    op(
        SmartPlaylistRuleOperator::NotEquals,
        SmartPlaylistRuleValueKind::Text,
    ),
    op(
        SmartPlaylistRuleOperator::IsEmpty,
        SmartPlaylistRuleValueKind::None,
    ),
    op(
        SmartPlaylistRuleOperator::IsNotEmpty,
        SmartPlaylistRuleValueKind::None,
    ),
];

const GENRE_OPS: [SmartPlaylistRuleOp; 4] = [
    op(
        SmartPlaylistRuleOperator::Contains,
        SmartPlaylistRuleValueKind::Text,
    ),
    op(
        SmartPlaylistRuleOperator::Equals,
        SmartPlaylistRuleValueKind::Text,
    ),
    op(
        SmartPlaylistRuleOperator::NotContains,
        SmartPlaylistRuleValueKind::Text,
    ),
    op(
        SmartPlaylistRuleOperator::NotEquals,
        SmartPlaylistRuleValueKind::Text,
    ),
];

const RATING_OPS: [SmartPlaylistRuleOp; 6] = [
    op(
        SmartPlaylistRuleOperator::Above,
        SmartPlaylistRuleValueKind::Number,
    ),
    op(
        SmartPlaylistRuleOperator::Below,
        SmartPlaylistRuleValueKind::Number,
    ),
    op(
        SmartPlaylistRuleOperator::Equals,
        SmartPlaylistRuleValueKind::Number,
    ),
    op(
        SmartPlaylistRuleOperator::Between,
        SmartPlaylistRuleValueKind::NumberRange,
    ),
    op(
        SmartPlaylistRuleOperator::IsEmpty,
        SmartPlaylistRuleValueKind::None,
    ),
    op(
        SmartPlaylistRuleOperator::IsNotEmpty,
        SmartPlaylistRuleValueKind::None,
    ),
];

const NUMBER_OPS: [SmartPlaylistRuleOp; 5] = [
    op(
        SmartPlaylistRuleOperator::Between,
        SmartPlaylistRuleValueKind::NumberRange,
    ),
    op(
        SmartPlaylistRuleOperator::Above,
        SmartPlaylistRuleValueKind::Number,
    ),
    op(
        SmartPlaylistRuleOperator::Below,
        SmartPlaylistRuleValueKind::Number,
    ),
    op(
        SmartPlaylistRuleOperator::Equals,
        SmartPlaylistRuleValueKind::Number,
    ),
    op(
        SmartPlaylistRuleOperator::NotEquals,
        SmartPlaylistRuleValueKind::Number,
    ),
];

const BOOL_OPS: [SmartPlaylistRuleOp; 2] = [
    op(
        SmartPlaylistRuleOperator::Is,
        SmartPlaylistRuleValueKind::Bool,
    ),
    op(
        SmartPlaylistRuleOperator::IsNot,
        SmartPlaylistRuleValueKind::Bool,
    ),
];

const DATE_OPS: [SmartPlaylistRuleOp; 6] = [
    op(
        SmartPlaylistRuleOperator::Between,
        SmartPlaylistRuleValueKind::DateRange,
    ),
    op(
        SmartPlaylistRuleOperator::After,
        SmartPlaylistRuleValueKind::Date,
    ),
    op(
        SmartPlaylistRuleOperator::Before,
        SmartPlaylistRuleValueKind::Date,
    ),
    op(
        SmartPlaylistRuleOperator::Equals,
        SmartPlaylistRuleValueKind::Date,
    ),
    op(
        SmartPlaylistRuleOperator::IsEmpty,
        SmartPlaylistRuleValueKind::None,
    ),
    op(
        SmartPlaylistRuleOperator::IsNotEmpty,
        SmartPlaylistRuleValueKind::None,
    ),
];

const fn op(
    operator: SmartPlaylistRuleOperator,
    value_kind: SmartPlaylistRuleValueKind,
) -> SmartPlaylistRuleOp {
    SmartPlaylistRuleOp {
        operator,
        value_kind,
    }
}

pub fn rule_fields() -> &'static [SmartPlaylistRuleField] {
    &RULE_FIELDS
}

pub fn sort_fields() -> &'static [SmartPlaylistSortField] {
    &SORT_FIELDS
}

pub fn rule_ops(field: SmartPlaylistRuleField) -> &'static [SmartPlaylistRuleOp] {
    match field {
        SmartPlaylistRuleField::Title
        | SmartPlaylistRuleField::Artist
        | SmartPlaylistRuleField::Album
        | SmartPlaylistRuleField::Comment => &TEXT_OPS,
        SmartPlaylistRuleField::Genre => &GENRE_OPS,
        SmartPlaylistRuleField::Rating => &RATING_OPS,
        SmartPlaylistRuleField::Year
        | SmartPlaylistRuleField::PlayCount
        | SmartPlaylistRuleField::SkipCount => &NUMBER_OPS,
        SmartPlaylistRuleField::Favorite | SmartPlaylistRuleField::Played => &BOOL_OPS,
        SmartPlaylistRuleField::LastPlayed | SmartPlaylistRuleField::DateAdded => &DATE_OPS,
    }
}

pub fn value_kind(
    field: SmartPlaylistRuleField,
    operator: SmartPlaylistRuleOperator,
) -> Option<SmartPlaylistRuleValueKind> {
    rule_ops(field)
        .iter()
        .find(|spec| spec.operator == operator)
        .map(|spec| spec.value_kind)
}

pub fn default_definition() -> SmartPlaylistDefinition {
    SmartPlaylistDefinition {
        root: SmartPlaylistRuleGroup {
            mode: SmartPlaylistMatchMode::All,
            rules: Vec::new(),
        },
        sort_field: SmartPlaylistSortField::Title,
        descending: false,
        limit: None,
    }
}

pub fn builtin_definition(builtin: SmartPlaylistBuiltin) -> SmartPlaylistDefinition {
    match builtin {
        SmartPlaylistBuiltin::MostPlayed => SmartPlaylistDefinition {
            root: group_all(vec![played_rule(true)]),
            sort_field: SmartPlaylistSortField::PlayCount,
            descending: true,
            limit: None,
        },
        SmartPlaylistBuiltin::NeverPlayed => SmartPlaylistDefinition {
            root: group_all(vec![played_rule(false)]),
            sort_field: SmartPlaylistSortField::Title,
            descending: false,
            limit: None,
        },
        SmartPlaylistBuiltin::MostSkipped => SmartPlaylistDefinition {
            root: group_all(vec![number_rule(
                SmartPlaylistRuleField::SkipCount,
                SmartPlaylistRuleOperator::Above,
                0,
            )]),
            sort_field: SmartPlaylistSortField::SkipCount,
            descending: true,
            limit: None,
        },
    }
}

pub fn default_rule(field: SmartPlaylistRuleField) -> SmartPlaylistRule {
    let operator = rule_ops(field)
        .first()
        .map(|spec| spec.operator)
        .unwrap_or(SmartPlaylistRuleOperator::Contains);
    SmartPlaylistRule {
        field,
        operator,
        value: default_value(field, operator),
    }
}

pub fn default_value(
    field: SmartPlaylistRuleField,
    operator: SmartPlaylistRuleOperator,
) -> Option<SmartPlaylistRuleValue> {
    match value_kind(field, operator) {
        Some(SmartPlaylistRuleValueKind::None) | None => None,
        Some(SmartPlaylistRuleValueKind::Text) => Some(SmartPlaylistRuleValue::Text(String::new())),
        Some(SmartPlaylistRuleValueKind::Number) => {
            Some(SmartPlaylistRuleValue::Number(number_bounds(field).2))
        }
        Some(SmartPlaylistRuleValueKind::NumberRange) => {
            let default = number_bounds(field).2;
            Some(SmartPlaylistRuleValue::NumberRange {
                min: default,
                max: default,
            })
        }
        Some(SmartPlaylistRuleValueKind::Date) => Some(SmartPlaylistRuleValue::Date(String::new())),
        Some(SmartPlaylistRuleValueKind::DateRange) => Some(SmartPlaylistRuleValue::DateRange {
            start: String::new(),
            end: String::new(),
        }),
        Some(SmartPlaylistRuleValueKind::Bool) => Some(SmartPlaylistRuleValue::Bool(true)),
    }
}

pub fn number_bounds(field: SmartPlaylistRuleField) -> (i64, i64, i64) {
    match field {
        SmartPlaylistRuleField::Rating => (0, 5, 4),
        SmartPlaylistRuleField::Year => (0, 3000, 2000),
        SmartPlaylistRuleField::PlayCount | SmartPlaylistRuleField::SkipCount => (0, 999_999, 1),
        _ => (0, 999_999, 0),
    }
}

pub fn normalize_root(group: &mut SmartPlaylistRuleGroup) {
    normalize_children(group);
}

pub fn normalize_group(group: &mut SmartPlaylistRuleGroup) -> Option<()> {
    normalize_children(group);
    (!group.rules.is_empty()).then_some(())
}

pub fn flat_rules(group: &SmartPlaylistRuleGroup) -> Option<Vec<SmartPlaylistRule>> {
    group
        .rules
        .iter()
        .map(|node| match node {
            SmartPlaylistRuleNode::Rule(rule) => Some(rule.clone()),
            SmartPlaylistRuleNode::Group(_) => None,
        })
        .collect()
}

pub fn text_value(rule: &SmartPlaylistRule) -> Option<String> {
    let SmartPlaylistRuleValue::Text(value) = rule.value.as_ref()? else {
        return None;
    };
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

pub fn number_value(rule: &SmartPlaylistRule) -> Option<i64> {
    let SmartPlaylistRuleValue::Number(value) = rule.value.as_ref()? else {
        return None;
    };
    Some(*value)
}

pub fn number_range_value(rule: &SmartPlaylistRule) -> Option<(i64, i64)> {
    let SmartPlaylistRuleValue::NumberRange { min, max } = rule.value.as_ref()? else {
        return None;
    };
    Some((*min, *max))
}

pub fn bool_value(rule: &SmartPlaylistRule) -> Option<bool> {
    let SmartPlaylistRuleValue::Bool(value) = rule.value.as_ref()? else {
        return None;
    };
    Some(*value)
}

pub fn date_value(rule: &SmartPlaylistRule) -> Option<String> {
    match rule.value.as_ref()? {
        SmartPlaylistRuleValue::Date(value) | SmartPlaylistRuleValue::Text(value) => {
            Some(value.trim().to_string())
        }
        SmartPlaylistRuleValue::Number(_)
        | SmartPlaylistRuleValue::NumberRange { .. }
        | SmartPlaylistRuleValue::Bool(_)
        | SmartPlaylistRuleValue::DateRange { .. } => None,
    }
    .filter(|value| !value.is_empty())
}

pub fn date_range_value(rule: &SmartPlaylistRule) -> Option<(String, String)> {
    match rule.value.as_ref()? {
        SmartPlaylistRuleValue::DateRange { start, end } => {
            let start = start.trim().to_string();
            let end = end.trim().to_string();
            if start.is_empty() || end.is_empty() {
                None
            } else if start <= end {
                Some((start, end))
            } else {
                Some((end, start))
            }
        }
        SmartPlaylistRuleValue::Text(_)
        | SmartPlaylistRuleValue::Number(_)
        | SmartPlaylistRuleValue::NumberRange { .. }
        | SmartPlaylistRuleValue::Bool(_)
        | SmartPlaylistRuleValue::Date(_) => None,
    }
}

fn normalize_children(group: &mut SmartPlaylistRuleGroup) {
    let mut normalized = Vec::with_capacity(group.rules.len());
    for mut node in group.rules.drain(..) {
        let keep = match &mut node {
            SmartPlaylistRuleNode::Group(group) => normalize_group(group).is_some(),
            SmartPlaylistRuleNode::Rule(rule) => normalize_rule(rule).is_some(),
        };
        if keep {
            normalized.push(node);
        }
    }
    group.rules = normalized;
}

fn normalize_rule(rule: &mut SmartPlaylistRule) -> Option<()> {
    match value_kind(rule.field, rule.operator)? {
        SmartPlaylistRuleValueKind::None => {
            rule.value = None;
            Some(())
        }
        SmartPlaylistRuleValueKind::Text => match rule.value.as_mut()? {
            SmartPlaylistRuleValue::Text(value) if !value.trim().is_empty() => {
                *value = value.trim().to_string();
                Some(())
            }
            _ => None,
        },
        SmartPlaylistRuleValueKind::Number => {
            matches!(rule.value, Some(SmartPlaylistRuleValue::Number(_))).then_some(())
        }
        SmartPlaylistRuleValueKind::NumberRange => {
            let Some(SmartPlaylistRuleValue::NumberRange { min, max }) = rule.value.as_mut() else {
                return None;
            };
            if *min > *max {
                std::mem::swap(min, max);
            }
            Some(())
        }
        SmartPlaylistRuleValueKind::Date => match rule.value.as_mut()? {
            SmartPlaylistRuleValue::Date(value) if !value.trim().is_empty() => {
                *value = value.trim().to_string();
                Some(())
            }
            _ => None,
        },
        SmartPlaylistRuleValueKind::DateRange => {
            let Some(SmartPlaylistRuleValue::DateRange { start, end }) = rule.value.as_mut() else {
                return None;
            };
            *start = start.trim().to_string();
            *end = end.trim().to_string();
            if start.is_empty() || end.is_empty() {
                return None;
            }
            if *start > *end {
                std::mem::swap(start, end);
            }
            Some(())
        }
        SmartPlaylistRuleValueKind::Bool => {
            matches!(rule.value, Some(SmartPlaylistRuleValue::Bool(_))).then_some(())
        }
    }
}

fn group_all(rules: Vec<SmartPlaylistRuleNode>) -> SmartPlaylistRuleGroup {
    SmartPlaylistRuleGroup {
        mode: SmartPlaylistMatchMode::All,
        rules,
    }
}

fn played_rule(played: bool) -> SmartPlaylistRuleNode {
    SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
        field: SmartPlaylistRuleField::Played,
        operator: SmartPlaylistRuleOperator::Is,
        value: Some(SmartPlaylistRuleValue::Bool(played)),
    })
}

fn number_rule(
    field: SmartPlaylistRuleField,
    operator: SmartPlaylistRuleOperator,
    value: i64,
) -> SmartPlaylistRuleNode {
    SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
        field,
        operator,
        value: Some(SmartPlaylistRuleValue::Number(value)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_playlist_normalizes_nested_rules() {
        let mut group = SmartPlaylistRuleGroup {
            mode: SmartPlaylistMatchMode::All,
            rules: vec![
                SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                    field: SmartPlaylistRuleField::Year,
                    operator: SmartPlaylistRuleOperator::Between,
                    value: Some(SmartPlaylistRuleValue::NumberRange {
                        min: 2001,
                        max: 1999,
                    }),
                }),
                SmartPlaylistRuleNode::Group(SmartPlaylistRuleGroup {
                    mode: SmartPlaylistMatchMode::Any,
                    rules: vec![SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                        field: SmartPlaylistRuleField::Genre,
                        operator: SmartPlaylistRuleOperator::Contains,
                        value: Some(SmartPlaylistRuleValue::Text(" rock ".to_string())),
                    })],
                }),
            ],
        };

        normalize_group(&mut group).expect("valid rules");

        let SmartPlaylistRuleNode::Rule(rule) = &group.rules[0] else {
            panic!("first node should be a rule");
        };
        assert_eq!(
            rule.value,
            Some(SmartPlaylistRuleValue::NumberRange {
                min: 1999,
                max: 2001,
            })
        );
        let SmartPlaylistRuleNode::Group(group) = &group.rules[1] else {
            panic!("second node should be a group");
        };
        let SmartPlaylistRuleNode::Rule(rule) = &group.rules[0] else {
            panic!("nested node should be a rule");
        };
        assert_eq!(
            rule.value,
            Some(SmartPlaylistRuleValue::Text("rock".to_string()))
        );
    }

    #[test]
    fn smart_playlist_normalizes_date_ranges() {
        let mut group = SmartPlaylistRuleGroup {
            mode: SmartPlaylistMatchMode::All,
            rules: vec![SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                field: SmartPlaylistRuleField::DateAdded,
                operator: SmartPlaylistRuleOperator::Between,
                value: Some(SmartPlaylistRuleValue::DateRange {
                    start: "2024-12-31".to_string(),
                    end: "2024-01-01".to_string(),
                }),
            })],
        };

        normalize_group(&mut group).expect("valid date range");

        let SmartPlaylistRuleNode::Rule(rule) = &group.rules[0] else {
            panic!("node should be a rule");
        };
        assert_eq!(
            rule.value,
            Some(SmartPlaylistRuleValue::DateRange {
                start: "2024-01-01".to_string(),
                end: "2024-12-31".to_string(),
            })
        );
    }

    #[test]
    fn smart_playlist_root_allows_empty_rules() {
        let mut group = SmartPlaylistRuleGroup {
            mode: SmartPlaylistMatchMode::All,
            rules: Vec::new(),
        };

        normalize_root(&mut group);

        assert!(group.rules.is_empty());
    }

    #[test]
    fn smart_playlist_drops_empty_value_rules() {
        let mut group = SmartPlaylistRuleGroup {
            mode: SmartPlaylistMatchMode::All,
            rules: vec![
                SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                    field: SmartPlaylistRuleField::Title,
                    operator: SmartPlaylistRuleOperator::Contains,
                    value: Some(SmartPlaylistRuleValue::Text(String::new())),
                }),
                SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                    field: SmartPlaylistRuleField::Genre,
                    operator: SmartPlaylistRuleOperator::Contains,
                    value: Some(SmartPlaylistRuleValue::Text("rock".to_string())),
                }),
            ],
        };

        normalize_group(&mut group).expect("valid remaining rule");

        assert_eq!(group.rules.len(), 1);
        let SmartPlaylistRuleNode::Rule(rule) = &group.rules[0] else {
            panic!("remaining node should be a rule");
        };
        assert_eq!(rule.field, SmartPlaylistRuleField::Genre);
    }

    #[test]
    fn smart_playlist_builtin_definitions_match_current_scope() {
        let most_played = builtin_definition(SmartPlaylistBuiltin::MostPlayed);
        assert_eq!(most_played.sort_field, SmartPlaylistSortField::PlayCount);
        assert!(most_played.descending);

        let never_played = builtin_definition(SmartPlaylistBuiltin::NeverPlayed);
        assert_eq!(never_played.sort_field, SmartPlaylistSortField::Title);
        assert!(!never_played.descending);

        let most_skipped = builtin_definition(SmartPlaylistBuiltin::MostSkipped);
        assert_eq!(most_skipped.sort_field, SmartPlaylistSortField::SkipCount);
        assert!(most_skipped.descending);
    }
}
