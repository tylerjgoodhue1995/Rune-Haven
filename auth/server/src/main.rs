mod auth;
mod background;
mod cache;
mod ratelimit;
mod web;

fn main() {
    env_logger::init();
    {
        let mut db = auth::db().expect("Failed to create database connection for migrations");
        auth::init_db(&mut db).expect("Failed to initialize database");
    }
    background::background_thread();
    web::start();
}
