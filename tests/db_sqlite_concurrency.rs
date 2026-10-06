//! The CRuby Db shim's SQLite concurrency policy: the request read
//! snapshot, the in-process write permit, and checkpoints off the
//! request path (runtime/spinel/db_cruby.rb).
//!
//! Each check loads the real shim against a file database (WAL needs a
//! file) and drives it from several threads, the way Puma does. None
//! needs an emitted app: the policy lives entirely below the lowerer, in
//! `Db.exec` / `Db.prepare` / `Db.with_connection`.
//!
//! cargo test --test db_sqlite_concurrency

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::path::PathBuf;

fn run(name: &str, body: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("roundhouse-db-concurrency-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = format!(
        r#"
require {pool:?}
require {shim:?}
module ActiveRecord; class RecordNotUnique < StandardError; end; end
def check(what, ok)
  raise "FAILED: #{{what}}" unless ok
end
def count(table)
  st = Db.prepare("SELECT COUNT(*) FROM #{{table}}")
  Db.step?(st)
  n = Db.column_int(st, 0)
  Db.finalize(st)
  n
end
Db.configure({db:?}, pool_size: 4)
{body}
puts "OK"
"#,
        pool = root.join("runtime/ruby/active_record/connection_pool.rb"),
        shim = root.join("runtime/spinel/db_cruby.rb"),
        db = dir.join("test.sqlite3"),
    );
    std::fs::write(dir.join("check.rb"), script).unwrap();
    let out = emit_and_run::ruby()
        .arg("check.rb")
        .current_dir(&dir)
        .output()
        .expect("spawn ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.trim_end().ends_with("OK"),
        "{name} ({}):\n=== stdout ===\n{stdout}\n=== stderr ===\n{}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A GET's reads share one snapshot: another connection's commit is
/// invisible until the snapshot ends. A write from inside the request
/// still succeeds — without ending the snapshot first, SQLite refuses
/// the upgrade of a stale read at once with SQLITE_BUSY — and the read
/// after it sees both rows.
#[test]
fn a_get_reads_one_snapshot_and_may_still_write() {
    run(
        "snapshot",
        r#"
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY)")
Db.exec("INSERT INTO t VALUES (1)")
Db.with_connection do
  Db.read_snapshot_begin
  check("snapshot is lazy", !Db.current_dbh.transaction_active?)
  check("first read", count("t") == 1)
  check("snapshot opened by the first read", Db.current_dbh.transaction_active?)
  Thread.new { Db.with_connection { Db.exec("INSERT INTO t VALUES (2)") } }.join
  check("other connection's commit is invisible inside the snapshot", count("t") == 1)
  Db.exec("INSERT INTO t VALUES (3)")
  check("the read after a write sees every commit", count("t") == 3)
  Db.read_snapshot_end
  check("snapshot closed at the end", !Db.current_dbh.transaction_active?)
end
"#,
    );
}

/// The brackets nest (a test dispatching inside a lease, a job drained
/// inside a request) and only the outermost end closes the snapshot. A
/// request's own transaction is never joined or closed by it.
#[test]
fn snapshot_brackets_nest_and_leave_app_transactions_alone() {
    run(
        "nesting",
        r#"
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY)")
Db.with_connection do
  Db.read_snapshot_begin
  Db.read_snapshot_begin
  count("t")
  Db.read_snapshot_end
  check("inner end keeps the snapshot", Db.current_dbh.transaction_active?)
  Db.read_snapshot_end
  check("outer end closes it", !Db.current_dbh.transaction_active?)

  Db.read_snapshot_begin
  Db.exec("BEGIN")
  Db.exec("INSERT INTO t VALUES (1)")
  check("a read inside the app's transaction", count("t") == 1)
  Db.exec("COMMIT")
  check("the app's COMMIT closed its transaction", !Db.current_dbh.transaction_active?)
  Db.read_snapshot_end
end
"#,
    );
}

/// Writers in one process queue on the permit rather than racing for
/// SQLite's lock. With every connection's busy handler set to zero, a
/// second writer reaching SQLite would fail at once; none does.
#[test]
fn writers_queue_on_the_permit_not_on_sqlites_lock() {
    run(
        "permit",
        r#"
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER)")
Db.instance_variable_get(:@pool).free.each { |c| c.busy_handler_timeout = 0 }
errors = Queue.new
threads = 8.times.map do |i|
  Thread.new do
    Db.with_connection do
      25.times do |j|
        begin
          if j.even?
            Db.exec("INSERT INTO t (n) VALUES (#{i})")
          else
            Db.exec("BEGIN")
            Db.exec("INSERT INTO t (n) VALUES (#{i})")
            sleep 0.0005
            Db.exec("INSERT INTO t (n) VALUES (#{i})")
            Db.exec("COMMIT")
          end
        rescue => e
          errors << e
          Db.exec("ROLLBACK") rescue nil
        end
      end
    end
  end
end
threads.each(&:join)
check("no writer met SQLITE_BUSY (#{errors.size})", errors.empty?)
check("every row landed", count("t") == 8 * (13 + 12 * 2))
"#,
    );
}

/// A transaction abandoned by something `transaction`'s rescue does not
/// catch must not keep the permit (stopping every writer in the process)
/// or hand the next lease a connection mid-transaction.
#[test]
fn an_abandoned_transaction_releases_the_permit_at_lease_end() {
    run(
        "abandoned",
        r#"
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY)")
begin
  Db.with_connection do
    Db.exec("BEGIN")
    Db.exec("INSERT INTO t VALUES (1)")
    raise Exception, "not a StandardError"
  end
rescue Exception
end
writer = Thread.new { Db.with_connection { Db.exec("INSERT INTO t VALUES (2)") } }
check("another writer got the permit", !writer.join(5).nil?)
check("the abandoned insert rolled back", count("t") == 1)
"#,
    );
}

/// Once the server asks, serving connections stop checkpointing inside
/// COMMIT and a background thread copies the log into the database
/// file instead: the file grows without any request checkpointing.
#[test]
fn checkpoints_run_off_the_request_path() {
    run(
        "checkpoint",
        r#"
Db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, b BLOB)")
path = Db.instance_variable_get(:@path)
Db.checkpoint_in_background!
before = File.size(path)
Db.with_connection do
  st = Db.current_dbh.execute("PRAGMA wal_autocheckpoint")
  check("leased connection no longer autocheckpoints", st[0][0] == 0)
  200.times { Db.exec("INSERT INTO t (b) VALUES (randomblob(4096))") }
end
deadline = Time.now + 5
sleep 0.05 while File.size(path) <= before + 400_000 && Time.now < deadline
check("background checkpoint copied the log (#{before} -> #{File.size(path)})",
      File.size(path) > before + 400_000)
"#,
    );
}
