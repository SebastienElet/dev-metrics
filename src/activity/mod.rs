mod model;
mod output;

pub use model::*;

pub trait ActivitySource {
    fn collect_activity(
        &self,
        user: Option<&str>,
        period: Period,
    ) -> Result<Activity, Box<dyn std::error::Error>>;
}
