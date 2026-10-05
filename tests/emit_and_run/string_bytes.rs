//! Native Ruby emission keeps byte values, evaluation count, and block effects.
use super::emit_and_run;

/// Overlay only the primitive probe; the standard fixture supplies a complete
/// emitted boot so this regression does not depend on handcrafted Rails setup.
fn byte_probe() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write(
        "app/lib/byte_probe.rb",
        r#"class ByteProbe
  def initialize
    @calls = 0
  end
  def receiver
    @calls += 1
    "A\0é東🎉"
  end
  def values
    receiver.bytes
  end
  def calls
    @calls
  end
  def block_value
    "é".bytes { |byte| @calls += byte }
  end
end
"#,
    )
}

const ASSERTIONS: &str = r#"
require_relative "app/models/byte_probe"
probe = ByteProbe.new
raise "wrong bytes" unless probe.values == [65, 0, 195, 169, 230, 157, 177, 240, 159, 142, 137]
raise "receiver evaluated more than once" unless probe.calls == 1
raise "wrong block return" unless probe.block_value == "é"
raise "block disappeared" unless probe.calls == 365
puts "String#bytes emitted contract passed"
"#;

/// Both runtimes execute byte methods natively, but Spinel also consumes the
/// generated signature, so a passing CRuby run alone cannot certify this fix.
fn assert_byte_signatures(path: &std::path::Path) {
    let rbs = std::fs::read_to_string(path).unwrap();
    assert!(rbs.contains("def values: () -> Array[Integer]"), "{rbs}");
    assert!(rbs.contains("def block_value: () -> String"), "{rbs}");
}

/// Integer sums distinguish executed block effects from both a dropped block
/// and string-valued bytes, while the call counter exposes receiver duplication.
#[test]
fn string_bytes_preserves_values_and_block_effects() {
    let run = byte_probe().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert_byte_signatures(&run.emitted.join("sig/app/models/byte_probe.rbs"));
}

/// Compile the same source and consume its inferred byte-array RBS on Spinel.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn string_bytes_preserves_values_and_block_effects_on_spinel() {
    let run = byte_probe().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert_byte_signatures(&run.emitted.join("app/models/byte_probe.rbs"));
}
