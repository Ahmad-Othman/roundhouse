//! The CRuby overlay's memoized signed-value verification
//! (ruby_overlay/runtime/message_verifier_cruby.rb) answers exactly what
//! the shared verifier answers: a cached value still expires on time, a
//! forged or re-purposed value is still rejected, and a rejection is
//! never cached.

use std::path::Path;
use std::process::Command;

#[test]
fn memoized_verification_matches_the_shared_verifier() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r##"
require "base64"
require_relative "runtime/spinel/message_digest_cruby"
require_relative "runtime/ruby/action_controller/message_verifier"
MV = ActionController::MessageVerifier
shared = MV.method(:verified_json)
require_relative "runtime/spinel/scaffold/ruby_overlay/runtime/message_verifier_cruby"

secret = "s" * 64
salt = "signed cookie"
good = MV.generate(secret, salt, "user-42", "cookie.session_token", true)
2.times do
  raise "good #{MV.verified(secret, salt, good, "cookie.session_token", true).inspect}" unless MV.verified(secret, salt, good, "cookie.session_token", true) == "user-42"
end
raise "cached a different answer" unless MV.verified_json(secret, salt, good, "cookie.session_token", true) == shared.call(secret, salt, good, "cookie.session_token", true)

# Another purpose, a tampered signature, another salt: rejected, uncached.
raise "purpose" unless MV.verified_json(secret, salt, good, "cookie.other", true) == ""
bad = good.sub(/.\z/) { |c| c == "a" ? "b" : "a" }
raise "forged" unless MV.verified_json(secret, salt, bad, "cookie.session_token", true) == ""
raise "salt" unless MV.verified_json(secret, "other salt", good, "cookie.session_token", true) == ""
raise "cached a rejection" if Thread.current[:rh_verified_json].values.any? { |v| v[0] == "" }

# An expiring value verifies while live, from the cache, and stops at exp.
soon = MV.iso8601_ms(Time.now + 1)
short = MV.envelope(secret, salt, MV.json_string("brief"), "cookie.flash", "\"#{soon}\"", true)
raise "live" unless MV.verified(secret, salt, short, "cookie.flash", true) == "brief"
raise "live cached" unless MV.verified(secret, salt, short, "cookie.flash", true) == "brief"
sleep 1.2
raise "expired hit still answered" unless MV.verified_json(secret, salt, short, "cookie.flash", true) == ""
puts "ALL OK"
"##;
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
        "memoized verification failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}
