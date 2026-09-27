# Minitest-shaped, like request_forgery_protection_test.rb: a CRuby-only
# framework test of the ruby family's signed session cookie
# (runtime/signed_session_cookie.rb over the shared MessageVerifier).
# A session survives its own round trip; anything the client could
# have written instead — an edit, a plaintext cookie, a value signed
# for another cookie name — restores as an empty session.
require "minitest/autorun"
require_relative "test_helper"
require_relative "../runtime/signed_session_cookie"

class SignedSessionCookieTest < Minitest::Test
  NAME = "_campfire_session"

  def signed_session(pairs)
    s = ActionDispatch::Session.new
    pairs.each { |k, v| s[k] = v }
    ActionDispatch::Session.signed_cookie(s.to_cookie, NAME)
  end

  def restored(raw, name = NAME)
    ActionDispatch::Session.from_signed_cookie(raw, name).to_cookie
  end

  def test_a_signed_session_round_trips
    raw = signed_session("_csrf_token" => "abc", "return_to" => "/rooms/1?x=a&b")
    assert_equal "_csrf_token=abc&return_to=%2Frooms%2F1%3Fx%3Da%26b", restored(raw)
  end

  def test_the_cookie_does_not_carry_the_plain_encoding
    refute_includes signed_session("_csrf_token" => "abc"), "_csrf_token="
  end

  def test_an_absent_or_plaintext_cookie_restores_empty
    assert_equal "", restored("")
    assert_equal "", restored("_csrf_token=planted")
  end

  def test_an_edited_payload_or_signature_restores_empty
    raw = signed_session("_csrf_token" => "abc")
    payload, digest = raw.split("--", 2)
    assert_equal "", restored(payload.reverse + "--" + digest)
    assert_equal "", restored(payload + "--" + digest.tr("0123456789abcdef", "123456789abcdef0"))
  end

  def test_a_value_signed_for_another_cookie_name_restores_empty
    assert_equal "", restored(signed_session("_csrf_token" => "abc"), "_other_session")
  end
end
