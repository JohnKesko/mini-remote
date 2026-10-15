//! Tests against a real SSH server (the test container that imitates a Raspberry Pi).
//!
//! They are marked `#[ignore]`; run them with
//! `cargo test -p mini-remote-ssh -- --include-ignored`.
//!
//! The server can be changed with the environment variables `MINI_REMOTE_TEST_SSH_HOST`,
//! `MINI_REMOTE_TEST_SSH_PORT`, `MINI_REMOTE_TEST_SSH_USER` and `MINI_REMOTE_TEST_SSH_PASSWORD`.
//! The defaults are the test values of the container (never use real accounts here).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ssh::{
    CancelToken, HostKeyCheck, HostKeyInfo, Progress, Sftp, SshAuth, SshConfig, SshError,
    SshSession,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The screen-sharing server in the container may serve only a few viewers at a time, so the
/// tests that use it take turns.
static VNC_TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The server refuses logins that arrive too many at once, and (OpenSSH 9.8 and newer) briefly
/// turns away an address that has just failed to log in. So the tests log in one by one, and wait
/// a few seconds after a failed login before the next one may start.
static LOGIN_TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn login(cfg: SshConfig, check: HostKeyCheck) -> Result<SshSession, SshError> {
    let _turn = LOGIN_TURN.lock().await;
    let result = SshSession::connect(cfg, check).await;
    if matches!(
        result,
        Err(SshError::AuthFailed | SshError::HostKeyRejected)
    ) {
        tokio::time::sleep(Duration::from_secs(6)).await;
    }
    result
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn config() -> SshConfig {
    SshConfig {
        host: env_or("MINI_REMOTE_TEST_SSH_HOST", "127.0.0.1"),
        port: env_or("MINI_REMOTE_TEST_SSH_PORT", "2222")
            .parse()
            .expect("port number"),
        username: env_or("MINI_REMOTE_TEST_SSH_USER", "pi"),
        auth: SshAuth::Password(env_or("MINI_REMOTE_TEST_SSH_PASSWORD", "openvnc-test")),
    }
}

fn accept_all() -> HostKeyCheck {
    Arc::new(|_info| Box::pin(async { true }))
}

async fn connect() -> SshSession {
    login(config(), accept_all())
        .await
        .expect("login to the test container")
}

fn no_progress() -> Progress {
    Box::new(|_, _| {})
}

/// A unique folder name so that several test runs do not disturb each other.
fn unique(prefix: &str) -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    )
}

fn local_scratch(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(unique(prefix));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Repeatable pseudo-random bytes, so that mix-ups between pieces would be noticed.
fn sample_bytes(len: usize) -> Vec<u8> {
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

/// Makes a fresh work folder in the remote home folder.
async fn remote_workdir(sftp: &Sftp) -> String {
    let home = sftp.home_dir().await.expect("home folder");
    let dir = format!("{home}/{}", unique("mini-remote-test"));
    sftp.mkdir(&dir).await.expect("make work folder");
    dir
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn login_reports_a_host_key_fingerprint() {
    let seen: Arc<Mutex<Vec<HostKeyInfo>>> = Arc::default();
    let sink = seen.clone();
    let check: HostKeyCheck = Arc::new(move |info| {
        sink.lock().unwrap().push(info);
        Box::pin(async { true })
    });
    let cfg = config();
    let session = login(cfg.clone(), check).await.expect("login");
    assert!(!session.is_closed());

    let info = {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "the callback runs exactly once");
        seen[0].clone()
    };
    assert_eq!(info.host, cfg.host);
    assert_eq!(info.port, cfg.port);
    assert!(
        info.algorithm.starts_with("ssh-") || info.algorithm.starts_with("ecdsa-"),
        "{}",
        info.algorithm
    );
    let hash = info
        .fingerprint
        .strip_prefix("SHA256:")
        .expect("starts with SHA256:");
    assert_eq!(
        hash.len(),
        43,
        "SHA-256 in base64 without padding is 43 letters: {hash}"
    );
    assert!(
        hash.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
    );
    // If OpenSSH's own tools are installed, they must report the very same fingerprint.
    if let Some(expected) = fingerprint_from_openssh_tools(&cfg, &info.algorithm) {
        assert_eq!(info.fingerprint, expected);
    } else {
        println!("OpenSSH tools not available: fingerprint not cross-checked");
    }
    session.close().await;
    assert!(session.is_closed());
}

/// Asks `ssh-keyscan` and `ssh-keygen -l` for the server's fingerprint.
fn fingerprint_from_openssh_tools(cfg: &SshConfig, algorithm: &str) -> Option<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let kind = if algorithm.starts_with("ecdsa") {
        "ecdsa"
    } else {
        algorithm.strip_prefix("ssh-")?
    };
    let scan = Command::new("ssh-keyscan")
        .args(["-t", kind, "-p", &cfg.port.to_string(), &cfg.host])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let mut keygen = Command::new("ssh-keygen")
        .args(["-l", "-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    keygen.stdin.take()?.write_all(&scan.stdout).ok()?;
    let out = keygen.wait_with_output().ok()?;
    String::from_utf8(out.stdout)
        .ok()?
        .split_whitespace()
        .find(|word| word.starts_with("SHA256:"))
        .map(str::to_string)
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn untrusted_host_key_stops_the_login() {
    let check: HostKeyCheck = Arc::new(|_| Box::pin(async { false }));
    let err = login(config(), check).await.expect_err("must not log in");
    assert!(matches!(err, SshError::HostKeyRejected), "{err:?}");
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn the_host_key_question_may_take_a_long_time() {
    // The person may think about it for longer than the connection time limits.
    let check: HostKeyCheck = Arc::new(|_| {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(25)).await;
            true
        })
    });
    SshSession::connect(config(), check)
        .await
        .expect("login after a slow answer");
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn wrong_password_is_reported_plainly() {
    let mut cfg = config();
    cfg.auth = SshAuth::Password("definitely-not-the-password".into());
    let err = login(cfg, accept_all()).await.expect_err("must not log in");
    assert!(matches!(err, SshError::AuthFailed), "{err:?}");
    assert!(err.to_string().contains("did not accept"));
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn nothing_listening_is_reported_plainly() {
    let mut cfg = config();
    cfg.port = 1; // nothing listens here
    let err = login(cfg, accept_all())
        .await
        .expect_err("must not connect");
    assert!(matches!(err, SshError::Network(_)), "{err:?}");
    println!("{err}");
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn tunnel_carries_the_screen_sharing_banner() {
    let _turn = VNC_TURN.lock().await;
    let session = connect().await;
    let mut tunnel = session
        .open_tunnel("localhost", 5900)
        .await
        .expect("tunnel");
    let mut banner = [0u8; 12];
    tokio::time::timeout(Duration::from_secs(10), tunnel.read_exact(&mut banner))
        .await
        .expect("banner in time")
        .expect("read banner");
    assert_eq!(&banner, b"RFB 003.008\n");
    // Answer the way a viewer does, to prove that data flows in both directions.
    tunnel.write_all(b"RFB 003.008\n").await.unwrap();
    let mut security = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(10), tunnel.read_exact(&mut security))
        .await
        .expect("security list in time")
        .expect("read security list");
    assert!(
        security[0] > 0,
        "the server offers at least one way to log in"
    );
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn tunnel_can_outlive_the_session_handle_and_many_can_be_open() {
    let _turn = VNC_TURN.lock().await;
    let session = connect().await;
    let mut a = session.open_tunnel("localhost", 5900).await.unwrap();
    let mut b = session.open_tunnel("localhost", 5900).await.unwrap();
    drop(session); // the tunnels keep the login alive
    let mut banner = [0u8; 12];
    a.read_exact(&mut banner).await.unwrap();
    assert_eq!(&banner, b"RFB 003.008\n");
    b.read_exact(&mut banner).await.unwrap();
    assert_eq!(&banner, b"RFB 003.008\n");
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn tunnel_to_a_closed_port_is_refused_plainly() {
    let _turn = VNC_TURN.lock().await;
    let session = connect().await;
    let err = session
        .open_tunnel("localhost", 9)
        .await
        .expect_err("nothing listens on port 9");
    assert!(matches!(err, SshError::TunnelRefused { .. }), "{err:?}");
    println!("{err}");
    // The login itself still works afterwards.
    assert!(session.open_tunnel("localhost", 5900).await.is_ok());
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn closing_the_session_ends_its_tunnels() {
    let _turn = VNC_TURN.lock().await;
    let session = connect().await;
    let mut tunnel = session.open_tunnel("localhost", 5900).await.unwrap();
    session.close().await;
    assert!(session.is_closed());
    let mut sink = Vec::new();
    // Either an error or the end of the stream; it must not hang.
    let finished = tokio::time::timeout(Duration::from_secs(10), async {
        let _ = tunnel.read_to_end(&mut sink).await;
    })
    .await;
    assert!(
        finished.is_ok(),
        "the tunnel must end after the session is closed"
    );
    assert!(matches!(
        session.open_tunnel("localhost", 5900).await,
        Err(SshError::Closed)
    ));
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn idle_tunnel_stays_alive_with_keepalives() {
    let _turn = VNC_TURN.lock().await;
    let session = connect().await;
    let mut tunnel = session.open_tunnel("localhost", 5900).await.unwrap();
    let mut banner = [0u8; 12];
    tunnel.read_exact(&mut banner).await.unwrap();
    tokio::time::sleep(Duration::from_secs(45)).await;
    assert!(!session.is_closed());
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn list_home_folder_is_sorted_with_folders_first() {
    let session = connect().await;
    let sftp = session.sftp().await.expect("file transfer");
    let home = sftp.home_dir().await.unwrap();
    assert!(home.starts_with('/'), "{home}");

    let work = remote_workdir(&sftp).await;
    sftp.mkdir(&format!("{work}/beta")).await.unwrap();
    sftp.mkdir(&format!("{work}/Alpha")).await.unwrap();
    for name in ["zeta.txt", "Apple.txt", "banana.txt"] {
        let local = local_scratch("list").join(name);
        std::fs::write(&local, b"x").unwrap();
        sftp.upload(
            &local,
            &format!("{work}/{name}"),
            no_progress(),
            CancelToken::default(),
        )
        .await
        .unwrap();
    }
    let entries = sftp.list_dir(&work).await.unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["Alpha", "beta", "Apple.txt", "banana.txt", "zeta.txt"]
    );
    assert!(entries[0].is_dir && entries[1].is_dir && !entries[2].is_dir);
    assert_eq!(entries[2].size, 1);
    assert_eq!(entries[2].path, format!("{work}/Apple.txt"));
    assert!(entries[2].modified.is_some());
    assert!(entries[2].permissions.is_some());
    assert!(!entries.iter().any(|e| e.name == "." || e.name == ".."));

    // The home folder itself is listable too.
    let home_entries = sftp.list_dir(&home).await.unwrap();
    assert!(home_entries.iter().any(|e| e.path == work));

    sftp.remove(&work).await.unwrap();
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn upload_then_download_gives_the_same_bytes() {
    let session = connect().await;
    let sftp = session.sftp().await.unwrap();
    let work = remote_workdir(&sftp).await;
    let scratch = local_scratch("roundtrip");

    // Not a multiple of the piece size, and several pieces long.
    let data = sample_bytes(5 * 1024 * 1024 + 12_345);
    let source = scratch.join("source.bin");
    std::fs::write(&source, &data).unwrap();
    let remote = format!("{work}/data.bin");

    let reports: Arc<Mutex<Vec<(u64, u64)>>> = Arc::default();
    let sink = reports.clone();
    sftp.upload(
        &source,
        &remote,
        Box::new(move |d, t| sink.lock().unwrap().push((d, t))),
        CancelToken::default(),
    )
    .await
    .unwrap();
    {
        let reports = reports.lock().unwrap();
        assert!(reports.len() >= 2, "progress is reported");
        assert!(
            reports.windows(2).all(|w| w[0].0 <= w[1].0),
            "progress never goes backwards"
        );
        assert_eq!(
            *reports.last().unwrap(),
            (data.len() as u64, data.len() as u64)
        );
        assert!(reports.iter().all(|&(_, t)| t == data.len() as u64));
    }
    // Only the finished file is visible: no temporary leftovers.
    let names: Vec<String> = sftp
        .list_dir(&work)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, ["data.bin"]);
    let entry = &sftp.list_dir(&work).await.unwrap()[0];
    assert_eq!(entry.size, data.len() as u64);

    let target = scratch.join("copy.bin");
    let reports: Arc<Mutex<Vec<(u64, u64)>>> = Arc::default();
    let sink = reports.clone();
    sftp.download(
        &remote,
        &target,
        Box::new(move |d, t| sink.lock().unwrap().push((d, t))),
        CancelToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), data);
    assert_eq!(
        *reports.lock().unwrap().last().unwrap(),
        (data.len() as u64, data.len() as u64)
    );
    let leftovers: Vec<_> = std::fs::read_dir(&scratch)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers.len(),
        2,
        "only source.bin and copy.bin: {leftovers:?}"
    );

    // Uploading again replaces the old file.
    let smaller = sample_bytes(1000);
    std::fs::write(&source, &smaller).unwrap();
    sftp.upload(&source, &remote, no_progress(), CancelToken::default())
        .await
        .unwrap();
    sftp.download(&remote, &target, no_progress(), CancelToken::default())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), smaller);

    // An empty file works too.
    let empty = scratch.join("empty");
    std::fs::write(&empty, b"").unwrap();
    sftp.upload(
        &empty,
        &format!("{work}/empty"),
        no_progress(),
        CancelToken::default(),
    )
    .await
    .unwrap();
    let back = scratch.join("empty-back");
    sftp.download(
        &format!("{work}/empty"),
        &back,
        no_progress(),
        CancelToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(&back).unwrap().len(), 0);

    sftp.remove(&work).await.unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn transfer_speed_is_reasonable() {
    let session = connect().await;
    let sftp = session.sftp().await.unwrap();
    let work = remote_workdir(&sftp).await;
    let scratch = local_scratch("speed");
    let data = sample_bytes(64 * 1024 * 1024);
    let source = scratch.join("big.bin");
    std::fs::write(&source, &data).unwrap();
    let remote = format!("{work}/big.bin");

    let start = std::time::Instant::now();
    sftp.upload(&source, &remote, no_progress(), CancelToken::default())
        .await
        .unwrap();
    let up = start.elapsed();
    let start = std::time::Instant::now();
    let back = scratch.join("big-back.bin");
    sftp.download(&remote, &back, no_progress(), CancelToken::default())
        .await
        .unwrap();
    let down = start.elapsed();
    assert_eq!(std::fs::read(&back).unwrap(), data);
    let mbps = |d: std::time::Duration| 64.0 / d.as_secs_f64();
    println!(
        "upload {:.0} MiB/s, download {:.0} MiB/s",
        mbps(up),
        mbps(down)
    );
    sftp.remove(&work).await.unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn cancelled_transfers_clean_up() {
    let session = connect().await;
    let sftp = session.sftp().await.unwrap();
    let work = remote_workdir(&sftp).await;
    let scratch = local_scratch("cancel");
    let data = sample_bytes(48 * 1024 * 1024);
    let source = scratch.join("big.bin");
    std::fs::write(&source, &data).unwrap();
    let remote = format!("{work}/big.bin");

    // Cancel an upload after a little progress.
    let token = CancelToken::default();
    let flag = token.clone();
    let err = sftp
        .upload(
            &source,
            &remote,
            Box::new(move |done, _| {
                if done > 1024 * 1024 {
                    flag.cancel();
                }
            }),
            token,
        )
        .await
        .expect_err("cancelled");
    assert!(matches!(err, SshError::Cancelled), "{err:?}");
    assert!(
        sftp.list_dir(&work).await.unwrap().is_empty(),
        "no half-uploaded file is left"
    );

    // Cancel a download the same way.
    sftp.upload(&source, &remote, no_progress(), CancelToken::default())
        .await
        .unwrap();
    let token = CancelToken::default();
    let flag = token.clone();
    let target = scratch.join("down.bin");
    let err = sftp
        .download(
            &remote,
            &target,
            Box::new(move |done, _| {
                if done > 1024 * 1024 {
                    flag.cancel();
                }
            }),
            token,
        )
        .await
        .expect_err("cancelled");
    assert!(matches!(err, SshError::Cancelled), "{err:?}");
    assert!(!target.exists());
    let names: Vec<_> = std::fs::read_dir(&scratch)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "only the source file is left: {names:?}");

    // A token that is already cancelled stops at once.
    let token = CancelToken::default();
    token.cancel();
    let err = sftp
        .download(&remote, &target, no_progress(), token)
        .await
        .expect_err("cancelled");
    assert!(matches!(err, SshError::Cancelled));

    sftp.remove(&work).await.unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn make_rename_and_remove() {
    let session = connect().await;
    let sftp = session.sftp().await.unwrap();
    let work = remote_workdir(&sftp).await;

    sftp.mkdir(&format!("{work}/one")).await.unwrap();
    // Making the same folder again is a plain-English error.
    let err = sftp
        .mkdir(&format!("{work}/one"))
        .await
        .expect_err("already there");
    assert!(matches!(err, SshError::Sftp(_)), "{err:?}");
    println!("{err}");

    sftp.rename(&format!("{work}/one"), &format!("{work}/two"))
        .await
        .unwrap();
    let names: Vec<String> = sftp
        .list_dir(&work)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, ["two"]);

    // A folder with files and sub-folders is removed completely.
    let scratch = local_scratch("remove");
    let file = scratch.join("f.txt");
    std::fs::write(&file, b"hello").unwrap();
    sftp.mkdir(&format!("{work}/two/deeper")).await.unwrap();
    sftp.upload(
        &file,
        &format!("{work}/two/f.txt"),
        no_progress(),
        CancelToken::default(),
    )
    .await
    .unwrap();
    sftp.upload(
        &file,
        &format!("{work}/two/deeper/g.txt"),
        no_progress(),
        CancelToken::default(),
    )
    .await
    .unwrap();
    sftp.remove(&format!("{work}/two")).await.unwrap();
    assert!(sftp.list_dir(&work).await.unwrap().is_empty());

    // A single file too, and a missing one gives a plain error.
    sftp.upload(
        &file,
        &format!("{work}/f.txt"),
        no_progress(),
        CancelToken::default(),
    )
    .await
    .unwrap();
    sftp.remove(&format!("{work}/f.txt")).await.unwrap();
    let err = sftp
        .remove(&format!("{work}/f.txt"))
        .await
        .expect_err("gone");
    assert!(err.to_string().contains("not found"), "{err}");

    // Listing a missing folder.
    let err = sftp
        .list_dir(&format!("{work}/nope"))
        .await
        .expect_err("missing");
    assert!(matches!(err, SshError::Sftp(_)));

    sftp.remove(&work).await.unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn whole_folders_go_up_and_come_back() {
    let session = connect().await;
    let sftp = session.sftp().await.unwrap();
    let work = remote_workdir(&sftp).await;
    let scratch = local_scratch("folders");

    // Build a small tree: nested folders, an empty folder, many small files and one big file.
    let tree = scratch.join("tree");
    std::fs::create_dir_all(tree.join("a/b")).unwrap();
    std::fs::create_dir_all(tree.join("empty")).unwrap();
    let mut expected: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..30 {
        let rel = format!("a/small-{i}.txt");
        let bytes = sample_bytes(100 + i * 37);
        std::fs::write(tree.join(&rel), &bytes).unwrap();
        expected.push((rel, bytes));
    }
    let big = sample_bytes(3 * 1024 * 1024 + 7);
    std::fs::write(tree.join("a/b/big.bin"), &big).unwrap();
    expected.push(("a/b/big.bin".into(), big));
    std::fs::write(tree.join("top.txt"), b"top").unwrap();
    expected.push(("top.txt".into(), b"top".to_vec()));
    let total: u64 = expected.iter().map(|(_, b)| b.len() as u64).sum();

    let reports: Arc<Mutex<Vec<(u64, u64)>>> = Arc::default();
    let sink = reports.clone();
    sftp.upload_dir(
        &tree,
        &work,
        Box::new(move |d, t| sink.lock().unwrap().push((d, t))),
        CancelToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(*reports.lock().unwrap().last().unwrap(), (total, total));
    let top: Vec<String> = sftp
        .list_dir(&format!("{work}/tree"))
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(top, ["a", "empty", "top.txt"]);

    let restored_parent = scratch.join("restored");
    std::fs::create_dir_all(&restored_parent).unwrap();
    let reports: Arc<Mutex<Vec<(u64, u64)>>> = Arc::default();
    let sink = reports.clone();
    sftp.download_dir(
        &format!("{work}/tree"),
        &restored_parent,
        Box::new(move |d, t| sink.lock().unwrap().push((d, t))),
        CancelToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(*reports.lock().unwrap().last().unwrap(), (total, total));
    for (rel, bytes) in &expected {
        assert_eq!(
            &std::fs::read(restored_parent.join("tree").join(rel)).unwrap(),
            bytes,
            "{rel}"
        );
    }
    assert!(
        restored_parent.join("tree/empty").is_dir(),
        "empty folders are kept"
    );

    // Copying the same folder again merges into the existing one.
    sftp.upload_dir(&tree, &work, no_progress(), CancelToken::default())
        .await
        .unwrap();

    // Folders can be removed in one go.
    sftp.remove(&format!("{work}/tree")).await.unwrap();
    assert!(sftp.list_dir(&work).await.unwrap().is_empty());
    sftp.remove(&work).await.unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn several_tasks_can_share_one_file_session() {
    let session = connect().await;
    let sftp = session.sftp().await.unwrap();
    let work = remote_workdir(&sftp).await;
    let scratch = local_scratch("shared");

    let mut tasks = Vec::new();
    for i in 0..6usize {
        let sftp = sftp.clone();
        let work = work.clone();
        let scratch = scratch.clone();
        tasks.push(tokio::spawn(async move {
            let data = sample_bytes(300_000 + i * 1000);
            let src = scratch.join(format!("in-{i}"));
            std::fs::write(&src, &data).unwrap();
            let remote = format!("{work}/f{i}");
            sftp.upload(&src, &remote, Box::new(|_, _| {}), CancelToken::default())
                .await
                .unwrap();
            let dst = scratch.join(format!("out-{i}"));
            sftp.download(&remote, &dst, Box::new(|_, _| {}), CancelToken::default())
                .await
                .unwrap();
            assert_eq!(std::fs::read(&dst).unwrap(), data);
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    // A second file session over the same login works as well.
    let second = session.sftp().await.unwrap();
    assert_eq!(second.list_dir(&work).await.unwrap().len(), 6);
    sftp.remove(&work).await.unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn file_session_keeps_the_login_open() {
    let session = connect().await;
    let sftp = session.sftp().await.unwrap();
    drop(session);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(sftp.home_dir().await.is_ok());
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn key_agent_without_agent_gives_a_plain_error() {
    // Only meaningful when no agent is running; with an agent that holds no matching key the
    // login is refused instead. Either way the error must be one of the plain ones.
    let mut cfg = config();
    cfg.auth = SshAuth::Agent;
    let err = login(cfg, accept_all()).await.expect_err("no agent login");
    assert!(
        matches!(err, SshError::Agent(_) | SshError::AuthFailed),
        "{err:?}"
    );
    println!("{err}");
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn missing_key_file_gives_a_plain_error() {
    let mut cfg = config();
    cfg.auth = SshAuth::KeyFile {
        path: std::env::temp_dir().join("mini-remote-no-such-key"),
        passphrase: None,
    };
    let err = login(cfg, accept_all()).await.expect_err("no key");
    assert!(matches!(err, SshError::LocalFile(_)), "{err:?}");
    println!("{err}");
}

/// A go-between on this computer that can be told to cut the line, so that the behaviour when the
/// network breaks can be tested.
struct CuttableLine {
    port: u16,
    mode: tokio::sync::watch::Sender<Cut>,
}

#[derive(Clone, Copy, PartialEq)]
enum Cut {
    Working,
    /// Close both ends, like a cable being pulled out and the router saying so.
    Reset,
    /// Keep both ends open but pass nothing on, like a wifi network that is simply gone.
    Silent,
}

impl CuttableLine {
    async fn start(target: String) -> CuttableLine {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (mode, watch) = tokio::sync::watch::channel(Cut::Working);
        tokio::spawn(async move {
            while let Ok((mut client, _)) = listener.accept().await {
                let target = target.clone();
                let mut watch = watch.clone();
                tokio::spawn(async move {
                    let Ok(mut server) = tokio::net::TcpStream::connect(target).await else {
                        return;
                    };
                    tokio::select! {
                        _ = tokio::io::copy_bidirectional(&mut client, &mut server) => return,
                        _ = watch.changed() => {}
                    }
                    let mode = *watch.borrow();
                    if mode == Cut::Silent {
                        // Keep both ends open and say nothing.
                        std::future::pending::<()>().await;
                    }
                    // Otherwise both ends are dropped here, which closes the line.
                });
            }
        });
        CuttableLine { port, mode }
    }
}

fn through(line: &CuttableLine) -> SshConfig {
    let mut cfg = config();
    cfg.host = "127.0.0.1".into();
    cfg.port = line.port;
    cfg
}

fn target_address() -> String {
    let cfg = config();
    format!("{}:{}", cfg.host, cfg.port)
}

#[tokio::test]
#[ignore = "needs the test container"]
async fn a_broken_line_is_noticed_at_once() {
    let line = CuttableLine::start(target_address()).await;
    let session = login(through(&line), accept_all()).await.unwrap();
    let sftp = session.sftp().await.unwrap();
    assert!(!session.is_closed());
    line.mode.send(Cut::Reset).unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(session.is_closed(), "a reset line must show as closed");
    let err = sftp.home_dir().await.expect_err("line is cut");
    println!("{err}");
    assert!(matches!(
        session.open_tunnel("localhost", 5900).await,
        Err(SshError::Closed)
    ));
}

#[tokio::test]
#[ignore = "needs the test container; takes about a minute and a half"]
async fn a_silent_line_is_noticed_by_the_keepalives() {
    let line = CuttableLine::start(target_address()).await;
    let session = login(through(&line), accept_all()).await.unwrap();
    // Some quiet time first: the keepalives must not break a healthy idle line.
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert!(!session.is_closed());
    line.mode.send(Cut::Silent).unwrap();
    let cut_at = std::time::Instant::now();
    while !session.is_closed() {
        assert!(
            cut_at.elapsed() < Duration::from_secs(150),
            "the dead line was never noticed"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    println!("noticed after {:?}", cut_at.elapsed());
}
