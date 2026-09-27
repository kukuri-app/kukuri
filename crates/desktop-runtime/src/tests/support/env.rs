use super::super::*;

pub(crate) fn social_graph_propagation_timeout() -> Duration {
    kukuri_test_support::constrained_timeout(Duration::from_secs(30), Duration::from_secs(300))
}

pub(crate) fn seeded_dht_runtime_ready_timeout() -> Duration {
    kukuri_test_support::constrained_timeout(Duration::from_secs(20), Duration::from_secs(120))
}

pub(crate) fn runtime_replication_timeout() -> Duration {
    kukuri_test_support::constrained_timeout(Duration::from_secs(30), Duration::from_secs(180))
}

pub(crate) fn runtime_shutdown_timeout() -> Duration {
    kukuri_test_support::constrained_timeout(Duration::from_secs(15), Duration::from_secs(60))
}

pub(crate) fn delete_sqlite_artifacts(db_path: &Path) {
    for path in [
        db_path.to_path_buf(),
        db_path.with_extension("db-shm"),
        db_path.with_extension("db-wal"),
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("delete sqlite artifact {}: {error}", path.display()),
        }
    }
}
