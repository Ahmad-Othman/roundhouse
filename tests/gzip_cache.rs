//! GzipCache: identical identity HTML is deflated once.

use std::path::Path;
use std::process::Command;

#[test]
fn identical_bodies_gzip_once() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"

n = 0
orig = Zlib.method(:gzip)
Zlib.define_singleton_method(:gzip) do |raw|
  n += 1
  orig.call(raw)
end

body = "x" * 128
app = lambda { |_env| [200, { "content-type" => "text/html" }, [body]] }
wrapped = GzipCache.wrap(app)
env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "gzip" }
a = wrapped.call(env)
b = wrapped.call(env)
raise "status #{a[0]}" unless a[0] == 200
raise "encoding" unless a[1]["content-encoding"] == "gzip"
raise "vary" unless a[1]["vary"].to_s.include?("Accept-Encoding")
raise "body changed" unless a[2] == b[2]
raise "gzipped #{n} times" unless n == 1
raise "not smaller" unless a[2][0].bytesize < body.bytesize

id_env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "identity" }
id = wrapped.call(id_env)
raise "identity encoded" if id[1]["content-encoding"]
raise "identity body" unless id[2] == [body]

head = wrapped.call(env.merge("REQUEST_METHOD" => "HEAD"))
raise "HEAD gzipped" if head[1]["content-encoding"]

no_body = GzipCache.wrap(lambda { |_e| [204, { "content-type" => "text/html" }, ["y" * 128]] })
nb = no_body.call(env)
raise "204 gzipped" if nb[1]["content-encoding"]

q0 = wrapped.call(env.merge("HTTP_ACCEPT_ENCODING" => "gzip;q=0, identity"))
raise "q=0 gzipped" if q0[1]["content-encoding"]
q08 = wrapped.call(env.merge("HTTP_ACCEPT_ENCODING" => "gzip;q=0.8"))
raise "q=0.8 skipped" unless q08[1]["content-encoding"] == "gzip"
puts "ALL OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "gzip cache failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn distinct_bodies_do_not_share_a_gzip() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"

a_body = "a" * 128
b_body = "b" * 128
a = GzipCache.wrap(lambda { |_e| [200, { "content-type" => "text/html" }, [a_body]] })
b = GzipCache.wrap(lambda { |_e| [200, { "content-type" => "text/html" }, [b_body]] })
env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "gzip" }
ga = a.call(env)
gb = b.call(env)
raise "same gzip" if ga[2][0] == gb[2][0]
raise "a not gzip" unless ga[1]["content-encoding"] == "gzip"
raise "b not gzip" unless gb[1]["content-encoding"] == "gzip"
puts "ALL OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "distinct-body gzip cache failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn tep_gzip_cached_hits_on_digest_not_body_pointer() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require "digest"
require "zlib"
require_relative "runtime/spinel/tep/tep_core"

n = 0
orig = Zlib.method(:gzip)
Zlib.define_singleton_method(:gzip) do |raw|
  n += 1
  orig.call(raw)
end

a = "y" * 128
b = "y" * 128
raise "same object" if a.equal?(b)
ga = Tep.gzip_cached(a)
gb = Tep.gzip_cached(b)
raise "gzipped #{n} times" unless n == 1
raise "bodies differ" unless ga == gb
gc = Tep.gzip_cached("z" * 128)
raise "distinct collided" if gc == ga
raise "second body not gzipped" unless n == 2
puts "ALL OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "tep gzip cache failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn overlay_read_str_does_not_dup_a_cached_fragment() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/rails_cache"

store = Rails::MemoryStore.new
frag = "message-html" * 32
store.write_str("k", frag, 0)
hit = store.read_str("k")
raise "miss" if hit.nil?
raise "duped" unless hit.equal?(store.read_str("k"))
raise "mutated store" unless hit.frozen?
begin
  hit << "x"
  raise "frozen fragment was mutable"
rescue FrozenError
end
other = store.read("k")
raise "untyped read must still dup" if other.equal?(hit)
other << "x"
raise "store corrupted" unless store.read_str("k") == frag
# write (untyped) also freezes, so a later read_str cannot mutate
# the shared entry — CodeRabbit on #432.
store.write("k2", "plain")
hit2 = store.read_str("k2")
raise "write miss" if hit2.nil?
raise "write not frozen" unless hit2.frozen?
begin
  hit2 << "x"
  raise "write-path fragment was mutable"
rescue FrozenError
end
raise "write store corrupted" unless store.read_str("k2") == "plain"
puts "ALL OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "overlay read_str failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn join_body_does_not_copy_a_one_part_rack_body() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"
part = "x" * 128
out = GzipCache.join_body([part])
raise "copied" unless out.equal?(part)
raise "empty" unless GzipCache.join_body([]) == ""
raise "joined" unless GzipCache.join_body(["a", "b"]) == "ab"
puts "ALL OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "join_body failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn identical_fresh_strings_gzip_once_via_last_hit() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"

n = 0
orig = Zlib.method(:gzip)
Zlib.define_singleton_method(:gzip) do |raw|
  n += 1
  orig.call(raw)
end

a = "x" * 128
b = "x" * 128
raise "same object" if a.equal?(b)
app = lambda { |_env| [200, { "content-type" => "text/html" }, [a]] }
# Second call uses a different String of the same bytes — wrk's shape.
n_at = 0
wrapped = GzipCache.wrap(lambda { |_env|
  body = n_at == 0 ? a : b
  n_at += 1
  [200, { "content-type" => "text/html" }, [body]]
})
env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "gzip" }
wrapped.call(env)
wrapped.call(env)
raise "gzipped #{n} times" unless n == 1
# Keys are [String#hash, bytesize, CRC-32] — small, never the body itself.
keys = GzipCache.instance_variable_get(:@store).keys
raise "key holds a body #{keys.inspect}" unless keys.all? { |k| k.is_a?(Array) && k.length == 3 && k.all?(Integer) }
puts "ALL OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "last-hit gzip failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn last_hit_does_not_follow_a_mutated_source_string() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"

n = 0
orig = Zlib.method(:gzip)
Zlib.define_singleton_method(:gzip) do |raw|
  n += 1
  orig.call(raw)
end

body = "x" * 128
wrapped = GzipCache.wrap(lambda { |_env|
  [200, { "content-type" => "text/html" }, [body]]
})
env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "gzip" }
first = wrapped.call(env)
body.replace("y" * 128)
second = wrapped.call(env)
raise "gzipped #{n} times" unless n == 2
raise "mutated source reused gzip" if first[2][0] == second[2][0]
puts "ALL OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "last-hit snapshot failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

#[test]
fn digest_fallback_does_not_reuse_gzip_across_distinct_bodies() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
require "zlib"
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"

# Equal size, different bytes — would collide under CRC32+size alone if
# Zlib.crc32 happened to match; digest keys refuse wrong-body hits either way.
a_body = "a" * 256
b_body = "b" * 256
n = 0
orig = Zlib.method(:gzip)
Zlib.define_singleton_method(:gzip) do |raw|
  n += 1
  orig.call(raw)
end
env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "gzip" }
ga = GzipCache.wrap(lambda { |_e| [200, { "content-type" => "text/html" }, [a_body]] }).call(env)
# Clear last-hit so the second request must use the Hash fallback.
GzipCache.instance_variable_set(:@last_raw, nil)
GzipCache.instance_variable_set(:@last_gz, nil)
gb = GzipCache.wrap(lambda { |_e| [200, { "content-type" => "text/html" }, [b_body]] }).call(env)
raise "same gzip across distinct bodies" if ga[2][0] == gb[2][0]
raise "gzipped #{n} times" unless n == 2
# Same body again via fallback (last-hit still cleared) must hit the digest store.
GzipCache.instance_variable_set(:@last_raw, nil)
GzipCache.instance_variable_set(:@last_gz, nil)
ga2 = GzipCache.wrap(lambda { |_e| [200, { "content-type" => "text/html" }, [a_body.dup]] }).call(env)
raise "digest miss" unless ga2[2][0] == ga[2][0]
raise "gzipped again #{n}" unless n == 2
puts "ALL OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "digest fallback failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}

/// The splice: a page that varies per request (a fresh token in the
/// layout) around a large cached fragment inflates to exactly its body,
/// with the fragment deflated once across requests. Non-ASCII text, a
/// repeated page reusing the last splice, a nested fragment, and a
/// fragment the body doesn't contain (the whole-body path) are each
/// covered. Compared as bytes: `Zlib.gunzip` returns BINARY.
#[test]
fn spliced_gzip_inflates_to_the_body_and_deflates_a_fragment_once() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r#"
module Rails
  class MemoryStore
    def initialize; @d = {}; end
    def read_str(k); @d[k]; end
    def write_str(k, v, _ttl); @d[k] = v.dup.freeze; end
  end
end
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/gzip_cache"
raise "splice unavailable" unless GzipCache::SPLICE_OK

store = Rails::MemoryStore.new
msgs = (1..400).map { |i| "<div class=\"message\" id=\"m#{i}\"><p>Message #{i} — héllo #{i * 7}</p></div>\n" }.join
store.write_str("coll", msgs, 0)

fragment_deflates = 0
orig = GzipCache.method(:raw_deflate)
GzipCache.define_singleton_method(:raw_deflate) do |d, dict, lv|
  fragment_deflates += 1 if lv == Zlib::DEFAULT_COMPRESSION
  orig.call(d, dict, lv)
end

bodies = []
seq = 0
app = GzipCache.wrap(lambda { |e|
  tok = e["TOK"] || (seq += 1).to_s * 16
  body = +"<html><head><meta content=#{tok}></head><body>" << store.read_str("coll") << "<form><input value=#{tok}></form></body></html>"
  bodies << body.dup
  [200, { "content-type" => "text/html" }, [body]]
})
env = { "REQUEST_METHOD" => "GET", "HTTP_ACCEPT_ENCODING" => "gzip" }
3.times do |i|
  _, h, b = app.call(env)
  raise "encoding #{i}" unless h["content-encoding"] == "gzip"
  raise "length #{i}" unless h["content-length"] == b[0].bytesize.to_s
  raise "round trip #{i}" unless Zlib.gunzip(b[0]) == bodies[i].b
end
raise "fragment deflated #{fragment_deflates}x" unless fragment_deflates == 1
whole = Zlib.gzip(bodies.last).bytesize
_, _, last = app.call(env)
raise "spliced #{last[0].bytesize} B vs whole #{whole} B" unless last[0].bytesize < whole * 1.10

# The same page again (no per-request token) reuses the last splice.
GzipCache.instance_variable_set(:@last_raw, nil)
_, _, r1 = app.call(env.merge("TOK" => "same"))
GzipCache.instance_variable_set(:@last_raw, nil)
_, _, r2 = app.call(env.merge("TOK" => "same"))
raise "repeat not reused" unless r1[0].equal?(r2[0])
raise "repeat round trip" unless Zlib.gunzip(r2[0]) == bodies.last.b

# A fragment nested in a later one (a collection miss writes its members
# first) is found in order; the container is skipped.
member = store.write_str("m1", msgs[0, 6000], 0)
body = "pre-" + msgs + "-post"
raise "nested" unless Zlib.gunzip(GzipCache.splice(body, [member, store.read_str("coll")])) == body.b

# Not in the body: no splice, the whole-body path answers.
raise "spliced a stranger" unless GzipCache.splice("plain page " * 50, [store.read_str("coll")]).nil?
stray = GzipCache.wrap(lambda { |_e|
  GzipCache.note_fragment(store.read_str("coll"))
  [200, { "content-type" => "text/html" }, ["no fragment here " * 50]]
})
_, _, sb = stray.call(env)
raise "fallback round trip" unless Zlib.gunzip(sb[0]) == ("no fragment here " * 50).b
puts "ALL OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "spliced gzip failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}
