use odx_core::Core;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread")]
async fn locking_during_unlock_keeps_the_vault_locked() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("vault.sqlite3");
    Core::new(path.clone())
        .unwrap()
        .create("test-password")
        .await
        .unwrap();
    let core = Arc::new(Core::new(path).unwrap());
    let unlocking = tokio::spawn({
        let core = core.clone();
        async move { core.unlock("test-password").await }
    });
    // Lock always wins whether the KDF is running or finished.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    core.lock().await;
    let _ = unlocking.await.unwrap();
    assert_eq!(core.status().await, "locked");
}

#[tokio::test]
async fn unlocking_removes_expired_output_files() {
    use std::time::{Duration, SystemTime};
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("vault.sqlite3");
    Core::new(path.clone())
        .unwrap()
        .create("test-password")
        .await
        .unwrap();
    let outputs = tmp.path().join("outputs");
    std::fs::create_dir(&outputs).unwrap();
    let file = |name: String, age: Duration| {
        let path = outputs.join(name);
        std::fs::write(&path, b"x").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() - age)
            .unwrap();
        path
    };
    let day = Duration::from_secs(24 * 3600);
    let expired = file(format!("{}-old.tsv", uuid::Uuid::new_v4()), day);
    let fresh = file(format!("{}-new.tsv", uuid::Uuid::new_v4()), Duration::ZERO);
    let foreign = file("ajeno.txt".into(), 2 * day);

    let core = Core::new(path).unwrap();
    core.unlock("test-password").await.unwrap();
    // Run cleanup in a blocking thread without delaying unlock.
    for _ in 0..200 {
        if !expired.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!expired.exists(), "remove expired files on unlock");
    assert!(fresh.exists() && foreign.exists());
}

#[tokio::test]
async fn changing_the_master_password_rekeys_the_open_vault() {
    let tmp = tempfile::tempdir().unwrap();
    let core = Core::new(tmp.path().join("vault.sqlite3")).unwrap();
    core.create("old-password").await.unwrap();
    assert!(
        core.change_password("wrong-password", "new-password")
            .await
            .is_err()
    );
    core.change_password("old-password", "new-password")
        .await
        .unwrap();
    core.lock().await;
    assert!(core.unlock("old-password").await.is_err());
    core.unlock("new-password").await.unwrap();
    assert_eq!(core.status().await, "unlocked");
    core.lock().await;
    assert!(
        core.change_password("new-password", "x-password")
            .await
            .is_err(),
        "locked"
    );
}
