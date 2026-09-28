# Minitest-shaped, like hash_to_query_test.rb: a CRuby-only framework
# test of the ruby family's forgery check
# (runtime/request_forgery_protection.rb over the shared
# `ActionController::Base#verify_authenticity_token`). actionpack's
# rule, case by case: safe verbs pass, a token must match the session's
# in the param OR the X-CSRF-Token header, and a foreign or `null`
# Origin fails even with a good token.
require "minitest/autorun"
require_relative "test_helper"
require_relative "../runtime/request_forgery_protection"

class RequestForgeryProtectionTest < Minitest::Test
  # The harness turns the check off for app suites, as Rails' test.rb
  # does; this file is the one that tests it.
  def setup
    ActionController::Base.allow_forgery_protection = true
  end

  def teardown
    ActionController::Base.allow_forgery_protection = false
  end

  TOKEN = "k3yF0rTh1sS3ss10n-abcdefghijklmnopqrstuvwxyz0"

  def controller(method:, params: {}, headers: {}, session_token: TOKEN)
    env = { "REQUEST_METHOD" => method, "HTTP_HOST" => "chat.example.com" }
    headers.each { |k, v| env[k] = v }
    c = ActionController::Base.new
    c.session[:_csrf_token] = session_token unless session_token.nil?
    c.params = params
    ActionController::Current.request = ActionDispatch::Request.new(env, params)
    c
  end

  def test_safe_verbs_pass_without_a_token
    assert controller(method: "GET").verified_request?
    assert controller(method: "HEAD").verified_request?
  end

  def test_a_post_without_a_token_is_refused_with_422
    c = controller(method: "POST")
    refute c.verified_request?
    c.verify_authenticity_token
    assert_equal 422, c.status
    assert c.performed?
  end

  def test_the_param_or_the_header_carries_the_token
    assert controller(method: "POST", params: { "authenticity_token" => TOKEN }).verified_request?
    assert controller(method: "DELETE", headers: { "HTTP_X_CSRF_TOKEN" => TOKEN }).verified_request?
    c = controller(method: "PATCH", params: { "authenticity_token" => TOKEN })
    c.verify_authenticity_token
    refute c.performed?
  end

  def test_a_wrong_token_or_a_session_without_one_is_refused
    refute controller(method: "POST", params: { "authenticity_token" => TOKEN.reverse }).verified_request?
    refute controller(method: "POST", params: { "authenticity_token" => TOKEN[0, 10] }).verified_request?
    refute controller(method: "POST", params: { "authenticity_token" => "" }, session_token: nil).verified_request?
  end

  def test_a_foreign_or_null_origin_is_refused_even_with_the_token
    ok = { "authenticity_token" => TOKEN }
    assert controller(method: "POST", params: ok, headers: { "HTTP_ORIGIN" => "https://chat.example.com" }).verified_request?
    refute controller(method: "POST", params: ok, headers: { "HTTP_ORIGIN" => "https://evil.example" }).verified_request?
    refute controller(method: "POST", params: ok, headers: { "HTTP_ORIGIN" => "null" }).verified_request?
  end

  # Action Cable's handshake check: same host, or any localhost port in
  # development; an absent Origin is refused, unlike the form check.
  def test_a_cable_handshake_must_come_from_its_own_host
    rfp = ActionController::RequestForgeryProtection
    host = "chat.example.com:3000"
    assert rfp.cable_origin_allowed?("http://chat.example.com:3000", host, false)
    assert rfp.cable_origin_allowed?("https://chat.example.com:3000", host, false)
    refute rfp.cable_origin_allowed?("https://evil.example", host, false)
    refute rfp.cable_origin_allowed?("http://chat.example.com:4000", host, false)
    refute rfp.cable_origin_allowed?("", host, false)
    refute rfp.cable_origin_allowed?("null", host, false)
  end

  def test_development_also_allows_any_localhost_port
    rfp = ActionController::RequestForgeryProtection
    host = "chat.example.com"
    refute rfp.cable_origin_allowed?("http://localhost:3000", host, false)
    assert rfp.cable_origin_allowed?("http://localhost:3000", host, true)
    assert rfp.cable_origin_allowed?("https://localhost:8443", host, true)
    refute rfp.cable_origin_allowed?("http://localhost:3000.evil.example", host, true)
    refute rfp.cable_origin_allowed?("", host, true)
  end
end
