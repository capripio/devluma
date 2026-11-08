use std::sync::Mutex;

use super::*;
use crate::domain::catalog::{ValidationOptions, fixtures, parse_and_validate};
use crate::domain::operation::OperationStatus;
use crate::persistence::installations::fixtures::installed;
use crate::persistence::operations as ops;
use crate::persistence::services::NewService;
use crate::services::journal::test_support::RecordingSink;
use crate::services::supervisor::tests::RecordingStates;

/// `php -c <ini> -m` that reports problems from markers in the file it's given:
/// `BROKEN` → a syntax error naming the file, `LEGACY` → a startup warning.
struct FakePhp {
    calls: Mutex<Vec<String>>,
}

impl ExecutableProbe for FakePhp {
    fn run(&self, _exe: &Path, args: &[&str]) -> Result<String, String> {
        let ini = args
            .iter()
            .position(|a| *a == "-c")
            .map(|i| args[i + 1].to_owned())
            .unwrap_or_default();
        self.calls.lock().unwrap().push(ini.clone());
        let text = std::fs::read_to_string(&ini).unwrap_or_default();
        let mut out = String::new();
        if text.contains("BROKEN") {
            out.push_str(&format!(
                "PHP:  syntax error, unexpected end of file in {ini} on line 2\n"
            ));
        }
        if text.contains("LEGACY") {
            out.push_str("Warning: PHP Startup: an old problem in Unknown on line 0\n");
        }
        out.push_str("[PHP Modules]\nCore\n");
        Ok(out)
    }
}

struct World {
    _dir: tempfile::TempDir,
    db: Arc<Database>,
    config: Arc<ConfigService>,
    php_dir: PathBuf,
    php: ConfigTarget,
    service: ConfigTarget,
    service_dir: PathBuf,
}

const INI: &str = "[PHP]\r\nmemory_limit = 128M\r\n;extension=curl\r\nextension=mbstring\r\n";

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths::under(dir.path());
    paths.create_all().unwrap();
    let db = Arc::new(crate::persistence::test_support::db());

    let php_dir = paths.packages.join("php").join("8.5.11");
    std::fs::create_dir_all(php_dir.join("ext")).unwrap();
    for dll in ["php_curl.dll", "php_mbstring.dll", "php_zend_test.dll"] {
        std::fs::write(php_dir.join("ext").join(dll), b"").unwrap();
    }
    std::fs::write(php_dir.join("php.ini"), INI).unwrap();
    let svc_pkg = dir.path().join("svc-pkg");
    let (php_id, service_id) = db
        .write(|c| {
            let php = installed(c, "php", "8.5.11", &php_dir.to_string_lossy())?;
            let svc = installed(c, "svc", "1.0.0", &svc_pkg.to_string_lossy())?;
            let s = crate::persistence::services::create(
                c,
                &NewService {
                    package_installation_id: &svc.id,
                    instance_name: "fake",
                    service_type: "web_server",
                    lifecycle_adapter: "fixture",
                    port: Some(18_080),
                    ..Default::default()
                },
            )?;
            Ok((php.id, s.id))
        })
        .unwrap();

    let catalog = Arc::new(
        parse_and_validate(&fixtures::document(vec![]), ValidationOptions::default()).unwrap(),
    );
    let sink: Arc<dyn ProgressSink> = Arc::new(RecordingSink::default());
    let sup = Supervisor::new(
        Arc::clone(&db),
        catalog,
        paths.clone(),
        Arc::clone(&sink),
        Arc::new(RecordingStates::default()),
    );
    let config = ConfigService::new(
        Arc::clone(&db),
        sup,
        Arc::new(FakePhp {
            calls: Mutex::new(Vec::new()),
        }),
        sink,
    );
    World {
        service_dir: paths.services.join(&service_id),
        _dir: dir,
        db,
        config,
        php_dir,
        php: ConfigTarget::Php {
            installation_id: php_id,
        },
        service: ConfigTarget::Service { service_id },
    }
}

impl World {
    fn ini(&self) -> String {
        std::fs::read_to_string(self.php_dir.join("php.ini")).unwrap()
    }

    fn last_op(&self) -> crate::persistence::operations::Operation {
        self.db
            .read(|c| ops::list_for_display(c, 1))
            .unwrap()
            .remove(0)
    }

    fn file_change(&self, target: &ConfigTarget, key: &str, text: &str) -> ConfigChange {
        let view = self.config.view(target).unwrap();
        let file = view.files.iter().find(|f| f.key == key).unwrap();
        ConfigChange::File {
            file: key.into(),
            text: text.into(),
            base_hash: file.hash.clone(),
        }
    }
}

fn settings(values: &[(&str, Option<&str>)], extensions: &[(&str, bool)]) -> ConfigChange {
    ConfigChange::Settings {
        values: values
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.map(str::to_owned)))
            .collect(),
        extensions: extensions
            .iter()
            .map(|(k, v)| ((*k).to_owned(), *v))
            .collect(),
    }
}

#[test]
fn php_view_reads_settings_and_extensions_from_php_ini() {
    let w = world();
    let view = w.config.view(&w.php).unwrap();
    assert_eq!(view.settings_file.as_deref(), Some("main"));
    assert!(view.settings_unavailable.is_none());
    let mem = view
        .settings
        .iter()
        .find(|s| s.spec.key == "memory_limit")
        .unwrap();
    assert_eq!(mem.value.as_deref(), Some("128"));
    let tz = view
        .settings
        .iter()
        .find(|s| s.spec.key == "date.timezone")
        .unwrap();
    assert_eq!(
        (tz.value.as_deref(), tz.raw.as_deref()),
        (None, None),
        "default"
    );
    let ext: Vec<(String, bool)> = view
        .extensions
        .unwrap()
        .into_iter()
        .map(|e| (e.name, e.enabled))
        .collect();
    assert_eq!(
        ext,
        vec![("curl".into(), false), ("mbstring".into(), true)],
        "test-only modules are hidden"
    );
    let file = &view.files[0];
    assert!(file.editable && file.read_only_reason.is_none());
    assert_eq!(file.text.as_deref(), Some(INI));
}

#[test]
fn php_settings_are_previewed_then_written_in_place_after_php_accepts_them() {
    let w = world();
    let change = settings(
        &[
            ("memory_limit", Some("512")),
            ("date.timezone", Some("Asia/Karachi")),
        ],
        &[("curl", true)],
    );
    let preview = w.config.preview(&w.php, &change).unwrap();
    assert!(preview.diff.contains("-memory_limit = 128M"));
    assert!(preview.diff.contains("+memory_limit = 512M"));
    assert!(preview.diff.contains("+extension=curl"));
    assert_eq!(w.ini(), INI, "a preview changes nothing");

    let applied = w.config.apply(&w.php, &change).unwrap();
    assert_eq!(
        w.ini(),
        "[PHP]\r\nmemory_limit = 512M\r\nextension=curl\r\nextension=mbstring\r\ndate.timezone = \"Asia/Karachi\"\r\n"
    );
    assert!(applied.restart.is_empty());
    assert_eq!(applied.view.files[0].backups.len(), 1);
    assert_eq!(
        w.config
            .read_backup(&w.php, "main", &applied.view.files[0].backups[0].id)
            .unwrap(),
        INI.replace("\r\n", "\n")
    );
    assert!(!w.php_dir.join("php.ini.devluma-new").exists());
    let op = w.last_op();
    assert_eq!(
        (op.operation_type.as_str(), op.status),
        ("edit_config", OperationStatus::Succeeded)
    );

    // Back to the default: the line is commented out.
    w.config
        .apply(&w.php, &settings(&[("memory_limit", None)], &[]))
        .unwrap();
    assert!(w.ini().contains(";memory_limit = 512M\r\n"));
}

#[test]
fn php_problems_block_the_save_and_leave_php_ini_untouched() {
    let w = world();
    let change = w.file_change(&w.php, "main", "[PHP]\nmemory_limit = BROKEN\n");
    let err = w.config.apply(&w.php, &change).unwrap_err();
    assert_eq!(err.code, ErrorCode::ConfigInvalid);
    assert!(
        err.message.contains("php.ini on line 2") && !err.message.contains("devluma-new"),
        "{}",
        err.message
    );
    assert_eq!(w.ini(), INI);
    assert!(!w.php_dir.join("php.ini.devluma-new").exists());
    assert!(w.config.view(&w.php).unwrap().files[0].backups.is_empty());
    assert_eq!(w.last_op().status, OperationStatus::Failed);

    // A problem the file already had doesn't block other changes.
    std::fs::write(
        w.php_dir.join("php.ini"),
        "x = LEGACY\nmemory_limit = 128M\n",
    )
    .unwrap();
    w.config
        .apply(&w.php, &settings(&[("memory_limit", Some("256"))], &[]))
        .unwrap();
    assert!(w.ini().contains("memory_limit = 256M"));
}

#[test]
fn editor_saves_keep_line_endings_and_refuse_stale_or_secret_files() {
    let w = world();
    let change = w.file_change(&w.php, "main", "[PHP]\nmemory_limit = 64M\n");
    // Someone edits the file in another editor meanwhile.
    std::fs::write(w.php_dir.join("php.ini"), "[PHP]\r\nmemory_limit = 96M\r\n").unwrap();
    let err = w.config.apply(&w.php, &change).unwrap_err();
    assert!(err.message.contains("changed on disk"), "{}", err.message);

    let change = w.file_change(&w.php, "main", "[PHP]\nmemory_limit = 64M\n");
    w.config.apply(&w.php, &change).unwrap();
    assert_eq!(w.ini(), "[PHP]\r\nmemory_limit = 64M\r\n", "CRLF kept");

    std::fs::write(
        w.php_dir.join("php.ini"),
        "smtp_password = hunter22secret\n",
    )
    .unwrap();
    let view = w.config.view(&w.php).unwrap();
    let file = &view.files[0];
    assert!(file.read_only_reason.is_some());
    assert!(!file.text.as_deref().unwrap().contains("hunter22"));
    let err = w
        .config
        .apply(
            &w.php,
            &ConfigChange::File {
                file: "main".into(),
                text: "smtp_password = [REDACTED]\n".into(),
                base_hash: file.hash.clone(),
            },
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ValidationError);
    assert!(
        w.ini().contains("hunter22secret"),
        "the secret was not overwritten"
    );
}

#[test]
fn invalid_settings_and_unknown_extensions_are_refused() {
    let w = world();
    for change in [
        settings(&[("memory_limit", Some("lots"))], &[]),
        settings(&[("not_a_setting", Some("1"))], &[]),
        settings(&[("date.timezone", Some("x\"\nextension=evil"))], &[]),
        settings(&[], &[("evil", true)]),
    ] {
        let err = w.config.apply(&w.php, &change).unwrap_err();
        assert_eq!(err.code, ErrorCode::ValidationError, "{}", err.message);
    }
    assert_eq!(w.ini(), INI);

    std::fs::remove_file(w.php_dir.join("php.ini")).unwrap();
    let view = w.config.view(&w.php).unwrap();
    assert!(view.settings_unavailable.unwrap().contains("no php.ini"));
    assert!(
        w.config
            .apply(&w.php, &settings(&[("memory_limit", Some("256"))], &[]))
            .is_err()
    );
}

#[test]
fn service_files_are_checked_in_place_and_restored_when_rejected() {
    let w = world();
    let view = w.config.view(&w.service).unwrap();
    let keys: Vec<(&str, bool)> = view
        .files
        .iter()
        .map(|f| (f.key.as_str(), f.editable))
        .collect();
    assert_eq!(keys, vec![("custom", true), ("main", false)]);

    let custom = w.service_dir.join("custom.conf");
    let change = w.file_change(&w.service, "custom", "good = 1\n");
    let applied = w.config.apply(&w.service, &change).unwrap();
    assert!(
        applied.message.contains("next time it starts"),
        "{}",
        applied.message
    );
    assert_eq!(std::fs::read_to_string(&custom).unwrap(), "good = 1\n");

    let change = w.file_change(&w.service, "custom", "this is invalid\n");
    let err = w.config.apply(&w.service, &change).unwrap_err();
    assert_eq!(err.code, ErrorCode::ConfigInvalid);
    assert!(
        err.message.contains("Nothing was changed"),
        "{}",
        err.message
    );
    assert_eq!(
        std::fs::read_to_string(&custom).unwrap(),
        "good = 1\n",
        "the previous file is back"
    );
    let backups = &w.config.view(&w.service).unwrap().files[0].backups;
    assert_eq!(
        backups.len(),
        1,
        "custom.conf didn't exist before the first save"
    );

    // Generated files can't be edited.
    let change = w.file_change(&w.service, "main", "port=1\n");
    assert_eq!(
        w.config.apply(&w.service, &change).unwrap_err().code,
        ErrorCode::ValidationError
    );
}

#[test]
fn managed_settings_are_stored_rendered_and_reset() {
    let w = world();
    let change = settings(&[("limit", Some("50"))], &[]);
    let preview = w.config.preview(&w.service, &change).unwrap();
    assert!(preview.diff.contains("+limit=50"), "{}", preview.diff);

    w.config.apply(&w.service, &change).unwrap();
    assert_eq!(
        std::fs::read_to_string(w.service_dir.join("fake.conf")).unwrap(),
        "port=18080\nlimit=50\n"
    );
    let view = w.config.view(&w.service).unwrap();
    assert_eq!(view.settings[0].value.as_deref(), Some("50"));
    assert!(view.settings_file.is_none());

    assert!(
        w.config
            .apply(&w.service, &settings(&[("limit", Some("500"))], &[]))
            .is_err(),
        "outside 1–100"
    );

    // A failed check puts the previous settings back.
    std::fs::write(w.service_dir.join("custom.conf"), "invalid\n").unwrap();
    let err = w
        .config
        .apply(&w.service, &settings(&[("limit", Some("60"))], &[]))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ConfigInvalid);
    let ServiceCtx {
        settings: stored, ..
    } = clone_ctx(
        &w.config
            .supervisor
            .config_context(match &w.service {
                ConfigTarget::Service { service_id } => service_id,
                ConfigTarget::Php { .. } => unreachable!(),
            })
            .unwrap()
            .1,
    );
    assert_eq!(stored.get("limit").map(String::as_str), Some("50"));
    std::fs::write(w.service_dir.join("custom.conf"), "\n").unwrap();

    w.config
        .apply(&w.service, &settings(&[("limit", None)], &[]))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(w.service_dir.join("fake.conf")).unwrap(),
        "port=18080\n"
    );
}

#[test]
fn backups_are_read_by_exact_name_only() {
    let w = world();
    for bad in [
        "../php.ini",
        "x.bak",
        "20261022T003320123Z.txt",
        "..\\20261022T003320123Z.bak",
    ] {
        assert!(w.config.read_backup(&w.php, "main", bad).is_err(), "{bad}");
    }
    assert!(is_backup_name("20261022T003320123Z.bak"));
}

// ---- live: the real tools --------------------------------------------------------------------

/// `cargo test live_config_editing -- --ignored` (set `DEVLUMA_LIVE_CACHE` to reuse downloads):
/// settings and editor saves against real PHP, nginx, Apache, MariaDB and PostgreSQL. Bad
/// files are rejected by each tool's own check and leave the previous file in place; good
/// ones are used by the running tool.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the network and real services"]
async fn live_config_editing() {
    use crate::platform::windows::process::run_captured;
    use crate::services::install::ProcessProbe;
    use crate::services::supervisor::tests::{LiveEnv, free_port};
    use std::time::Duration;

    let env = LiveEnv::install(&[
        ("php", "8.5.12-1"),
        ("nginx", "1.30.5"),
        ("apache", "2.4.70"),
        ("mariadb", "11.8.9"),
        ("postgresql", "18.6"),
    ])
    .await;
    let config = ConfigService::new(
        Arc::clone(&env.db),
        Arc::clone(&env.sup),
        Arc::new(ProcessProbe),
        Arc::new(RecordingSink::default()),
    );
    let change_file = |target: &ConfigTarget, key: &str, edit: &dyn Fn(&str) -> String| {
        let view = config.view(target).unwrap();
        let f = view.files.iter().find(|f| f.key == key).unwrap();
        ConfigChange::File {
            file: key.into(),
            text: edit(f.text.as_deref().unwrap_or("")),
            base_hash: f.hash.clone(),
        }
    };
    let rejected = |target: &ConfigTarget, change: ConfigChange, path: &Path, needle: &str| {
        let before = std::fs::read(path).unwrap();
        let err = config.apply(target, &change).unwrap_err();
        assert_eq!(err.code, ErrorCode::ConfigInvalid, "{}", err.message);
        assert!(err.message.contains(needle), "{needle}: {}", err.message);
        assert_eq!(std::fs::read(path).unwrap(), before, "previous file kept");
    };

    // PHP: php.ini from the official template, then settings and extensions.
    let php = env.installation("php");
    let php_dir = PathBuf::from(&php.install_path);
    php::create_ini(&ProcessProbe, &php_dir, php::IniTemplate::Development).unwrap();
    let php_t = ConfigTarget::Php {
        installation_id: php.id.clone(),
    };
    config
        .apply(
            &php_t,
            &settings(
                &[
                    ("memory_limit", Some("512")),
                    ("upload_max_filesize", Some("64")),
                    ("post_max_size", Some("64")),
                    ("date.timezone", Some("Asia/Karachi")),
                ],
                &[("curl", true), ("mbstring", true)],
            ),
        )
        .unwrap();
    let script = "echo ini_get('memory_limit'), '|', ini_get('upload_max_filesize'), '|', \
                  date_default_timezone_get(), '|', extension_loaded('curl') ? 'curl' : '-';";
    let out = run_captured(
        &php_dir.join("php.exe"),
        &["-r", script],
        Duration::from_secs(20),
    )
    .unwrap();
    assert_eq!(
        out.stdout.trim(),
        "512M|64M|Asia/Karachi|curl",
        "{}",
        out.stderr
    );
    let ini = php_dir.join("php.ini");
    rejected(
        &php_t,
        settings(&[("date.timezone", Some("Mars/Base"))], &[]),
        &ini,
        "Invalid date.timezone",
    );
    rejected(
        &php_t,
        change_file(&php_t, "main", &|t| {
            format!("{t}\nbroken = \"unterminated\n")
        }),
        &ini,
        "syntax error",
    );

    // nginx: settings render into nginx.conf, custom.conf is checked by nginx -t, and a
    // running server reloads.
    let web = env.create("nginx", "web", free_port(), None).await;
    let web_t = ConfigTarget::Service {
        service_id: web.clone(),
    };
    env.start(&web).await;
    let applied = config
        .apply(
            &web_t,
            &settings(
                &[("client_max_body_size", Some("64")), ("gzip", Some("on"))],
                &[],
            ),
        )
        .unwrap();
    assert!(applied.message.contains("reloaded"), "{}", applied.message);
    let custom = env
        .paths
        .services
        .join(&web)
        .join("conf")
        .join("custom.conf");
    rejected(
        &web_t,
        change_file(&web_t, "custom", &|_| "client_max_body_size 5m;\n".into()),
        &custom,
        "is duplicate",
    );
    rejected(
        &web_t,
        change_file(&web_t, "custom", &|_| "gzip_types text/css\n".into()),
        &custom,
        "custom.conf",
    );
    config
        .apply(
            &web_t,
            &change_file(&web_t, "custom", &|_| {
                "gzip_types text/css application/json;\n".into()
            }),
        )
        .unwrap();
    env.stop_cleanly(&web, true).await;

    // Apache: the same for httpd -t (not started).
    let apache = env.create("apache", "httpd", free_port(), None).await;
    let apache_t = ConfigTarget::Service {
        service_id: apache.clone(),
    };
    config
        .apply(
            &apache_t,
            &settings(
                &[("LimitRequestBody", Some("10")), ("KeepAlive", Some("off"))],
                &[],
            ),
        )
        .unwrap();
    rejected(
        &apache_t,
        change_file(&apache_t, "custom", &|_| "KeepAlive Maybe\n".into()),
        &env.paths
            .services
            .join(&apache)
            .join("conf")
            .join("custom.conf"),
        "KeepAlive must be On or Off",
    );

    // MariaDB: my.ini edited in place, checked by mariadbd, used after a (re)start.
    let db_port = free_port();
    let db = env.create("mariadb", "db", db_port, None).await;
    let db_t = ConfigTarget::Service {
        service_id: db.clone(),
    };
    config
        .apply(
            &db_t,
            &settings(
                &[
                    ("max_connections", Some("321")),
                    ("max_allowed_packet", Some("64")),
                ],
                &[],
            ),
        )
        .unwrap();
    let my_ini = PathBuf::from(
        env.db
            .read(|c| crate::persistence::services::get(c, &db))
            .unwrap()
            .data_path
            .unwrap(),
    )
    .join("my.ini");
    rejected(
        &db_t,
        change_file(&db_t, "main", &|t| {
            t.replacen("[mysqld]", "[mysqld]\nfoo_bar=1", 1)
        }),
        &my_ini,
        "unknown variable",
    );
    env.start(&db).await;
    let maria = env.installation("mariadb");
    let port = db_port.to_string();
    let out = run_captured(
        &PathBuf::from(&maria.install_path)
            .join("bin")
            .join("mariadb.exe"),
        &[
            "-h",
            "127.0.0.1",
            "-P",
            &port,
            "-u",
            "root",
            "-N",
            "-e",
            "SELECT @@max_connections, @@max_allowed_packet",
        ],
        Duration::from_secs(20),
    )
    .unwrap();
    assert_eq!(out.stdout.trim(), "321\t67108864", "{}", out.stderr);
    env.stop_cleanly(&db, true).await;

    // PostgreSQL: postgresql.conf edited in place, checked by postgres -C.
    let pg_port = free_port();
    let pg = env.create("postgresql", "pg", pg_port, None).await;
    let pg_t = ConfigTarget::Service {
        service_id: pg.clone(),
    };
    config
        .apply(
            &pg_t,
            &settings(
                &[
                    ("shared_buffers", Some("256")),
                    ("log_statement", Some("ddl")),
                ],
                &[],
            ),
        )
        .unwrap();
    let view = config.view(&pg_t).unwrap();
    let shared = view
        .settings
        .iter()
        .find(|s| s.spec.key == "shared_buffers")
        .unwrap();
    assert_eq!(shared.value.as_deref(), Some("256"));
    let conf = PathBuf::from(&view.files[0].path);
    rejected(
        &pg_t,
        change_file(&pg_t, "main", &|t| format!("{t}\nwork_mem = 4XB\n")),
        &conf,
        "invalid value for parameter",
    );
    env.start(&pg).await;
    let pg_inst = env.installation("postgresql");
    let port = pg_port.to_string();
    let out = run_captured(
        &PathBuf::from(&pg_inst.install_path)
            .join("bin")
            .join("psql.exe"),
        &[
            "-h",
            "127.0.0.1",
            "-p",
            &port,
            "-U",
            "postgres",
            "-tA",
            "-c",
            "SHOW shared_buffers",
        ],
        Duration::from_secs(20),
    )
    .unwrap();
    assert_eq!(out.stdout.trim(), "256MB", "{}", out.stderr);
    env.stop_cleanly(&pg, true).await;
}

/// `cargo test live_mongodb_settings -- --ignored` (~1 GB download; use `DEVLUMA_LIVE_CACHE`):
/// MongoDB's Settings tab against a real mongod. The rendered mongod.conf must start, and
/// mongod's own startup log must show the values it parsed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the network and real services"]
async fn live_mongodb_settings() {
    use crate::services::install::ProcessProbe;
    use crate::services::supervisor::tests::{LiveEnv, free_port};

    let env = LiveEnv::install(&[("mongodb", "9.0.2")]).await;
    let config = ConfigService::new(
        Arc::clone(&env.db),
        Arc::clone(&env.sup),
        Arc::new(ProcessProbe),
        Arc::new(RecordingSink::default()),
    );
    let mongo = env.create("mongodb", "mongo", free_port(), None).await;
    let target = ConfigTarget::Service {
        service_id: mongo.clone(),
    };
    let applied = config
        .apply(
            &target,
            &settings(
                &[
                    ("cache_size", Some("1536")),
                    ("slow_op_threshold_ms", Some("250")),
                    ("verbosity", Some("1")),
                ],
                &[],
            ),
        )
        .unwrap();
    assert!(
        applied.restart.is_empty(),
        "not running, so nothing to restart"
    );

    env.start(&mongo).await;
    let log = env.sup.service_dir(&mongo).join("logs").join("mongod.log");
    // mongod logs the options it parsed (one JSON line) and WiredTiger's open config.
    let text = std::fs::read_to_string(&log).unwrap();
    let options = text
        .lines()
        .find(|l| l.contains("\"Options set by command line\""))
        .unwrap_or_else(|| panic!("no options line in {}", log.display()));
    for needle in [
        "\"cacheSizeGB\":1.5",
        "\"slowOpThresholdMs\":250",
        "\"verbosity\":1",
        "\"bindIp\":\"127.0.0.1\"",
    ] {
        assert!(options.contains(needle), "{needle} missing in {options}");
    }
    assert!(
        text.contains("cache_size=1536M"),
        "WiredTiger opened with a 1.5 GB cache"
    );

    // Back to defaults: the keys disappear from mongod.conf.
    config
        .apply(
            &target,
            &settings(
                &[
                    ("cache_size", None),
                    ("slow_op_threshold_ms", None),
                    ("verbosity", None),
                ],
                &[],
            ),
        )
        .unwrap();
    let conf = std::fs::read_to_string(env.sup.service_dir(&mongo).join("mongod.conf")).unwrap();
    assert!(
        !conf.contains("cacheSizeGB") && !conf.contains("slowOpThresholdMs"),
        "{conf}"
    );
    env.stop_cleanly(&mongo, true).await;
}

#[test]
fn only_active_credentials_lock_a_file() {
    // initdb's postgresql.conf: names passwords, holds none.
    assert!(!holds_secret(
        "#password_encryption = scram-sha-256\t# or md5\npassword_encryption = scram-sha-256\n"
    ));
    assert!(!holds_secret("; mysqli.default_pw =\n#password=old\n"));
    assert!(holds_secret("[client]\npassword=hunter22secret\n"));
    assert!(holds_secret("smtp_password = hunter22secret\n"));
}
