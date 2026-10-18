use std::error::Error;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

pub(super) fn collect<T: Send, R: Send>(
    items: Vec<T>,
    fetch: impl Fn(T) -> Result<R, Box<dyn Error>> + Sync,
) -> Result<Vec<R>, Box<dyn Error>> {
    let workers = items.len().min(8);
    let items = Mutex::new(items.into_iter().enumerate());
    let failed = AtomicBool::new(false);
    let mut results = std::thread::scope(|scope| {
        let tasks = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut completed = Vec::new();
                    while !failed.load(Ordering::Acquire) {
                        let Some((index, item)) = items.lock().unwrap().next() else {
                            break;
                        };
                        let result = fetch(item).map_err(|error| error.to_string());
                        if result.is_err() {
                            failed.store(true, Ordering::Release);
                        }
                        completed.push((index, result));
                    }
                    completed
                })
            })
            .collect::<Vec<_>>();
        tasks
            .into_iter()
            .map(|task| task.join().map_err(|_| "Collecte GitHub interrompue"))
            .collect::<Result<Vec<_>, _>>()
    })?
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    results.sort_by_key(|(index, _)| *index);
    results
        .into_iter()
        .map(|(_, result)| result.map_err(Into::into))
        .collect()
}
