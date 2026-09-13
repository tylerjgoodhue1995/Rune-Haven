use std::time::Duration;

use crate::auth::db;

// Once every 2 hours
const BACKGROUND_JOB_INTERVAL: Duration = Duration::from_secs(60 * 60 * 2);

pub fn background_thread() {
    std::thread::Builder::new()
        .name("background-jobs".to_string())
        .spawn(move || {
            loop {
                std::thread::sleep(BACKGROUND_JOB_INTERVAL);
                // Panic here because if we cannot access the database, the server probably shouldn't continue running
                let db = match db() {
                    Ok(connection) => connection,
                    Err(error) => {
                        log::error!("Could not get database connection: {error:?}");
                        continue;
                    }
                };

                // Cleanup auth database
                if let Err(error) = crate::auth::cleanup(&db) {
                    log::error!("Error while running DB cleanup job: {error:?}");
                }
            }
        })
        .expect("Failed to start background job thread");
}
