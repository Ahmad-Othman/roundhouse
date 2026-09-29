# Driver for tests/spinel_db_lease.rs — COMPILED BY SPINEL: it loads
# runtime/spinel/db.rb, whose sqlite3 calls are spinel `ffi_func`s with
# no CRuby spelling. The harness copies db.rb (and the one file it
# requires) beside this one.
#
# A request that raises must give its connection back. `with_connection`
# released on the happy path only, so every 500 leaked one lease; after a
# pool's worth, the next lease on that shard parked forever and the
# binary stopped answering every route whose thread the shard served.
# scripts/campfire-http-shape found it on campfire.
#
# The probe raises MORE times than the pool holds, then reads how many
# connections are free and leases once more. On the leaking runtime the
# free count is 0 — the check fails before the final lease would hang.
require_relative "db"

POOL = 4

def check(name, ok)
  puts "#{ok ? "ok" : "FAIL"} #{name}"
end

path = ARGV[0] || "lease-probe.sqlite3"
Db.configure(path, pool_size: POOL)
Db.with_connection { 0 }

raised = 0
i = 0
while i < POOL + 2
  begin
    Db.with_connection { raise "boom" }
  rescue RuntimeError
    raised += 1
  end
  i += 1
end
check("every raise reached the caller", raised == POOL + 2)
check("no lease is held after a raise", Db.pool_for_thread.available == POOL)
check("the thread holds no connection after a raise", !Db.in_lease?)
check("a lease after the raises still serves", Db.with_connection { 42 } == 42)
puts "done"
