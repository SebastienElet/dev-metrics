use crate::activity::{ActivityWarning, Source, WarningCode, WarningSubject};
use std::collections::BTreeMap;
use std::error::Error;

pub(super) struct Batch<T> {
    pub items: BTreeMap<(String, u64), T>,
    pub warnings: Vec<ActivityWarning>,
}
impl<T> Batch<T> {
    pub fn empty() -> Self {
        Self {
            items: BTreeMap::new(),
            warnings: Vec::new(),
        }
    }
    pub fn extend(&mut self, batch: Self) {
        self.items.extend(batch.items);
        self.warnings.extend(batch.warnings);
    }
}

pub(super) fn item<T>(
    result: Result<T, Box<dyn Error>>,
    repository: &str,
    subject: WarningSubject,
    warnings: &mut Vec<ActivityWarning>,
) -> Result<Option<T>, Box<dyn Error>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if super::api::is_rate_limit(error.as_ref()) => Err(error),
        Err(error) => {
            warnings.push(warning(repository, subject, error.as_ref()));
            Ok(None)
        }
    }
}

pub(super) fn warning(
    repository: &str,
    subject: WarningSubject,
    error: &dyn Error,
) -> ActivityWarning {
    ActivityWarning {
        source: Source::GitHub,
        repository: repository.into(),
        subject,
        code: WarningCode::ItemUnavailable,
        message: format!("Élément ignoré ; rapport incomplet pour cet élément : {error}"),
    }
}
