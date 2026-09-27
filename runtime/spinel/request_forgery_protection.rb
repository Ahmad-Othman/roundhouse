# Rails' request forgery check: `verify_authenticity_token`, the target
# of the `before_action` that `protect_from_forgery with: :exception`
# registers (the lowering chains it where the app declares it — see
# `ingest::controller::parse_forgery_macro`). An unverified request is
# answered 422, which is what production Rails' `InvalidAuthenticityToken`
# becomes, and the synthesized dispatcher's halt check skips the action.
#
# actionpack's rule, `request_forgery_protection.rb`:
#
#   request.get? || request.head? || !protect_against_forgery? ||
#     (valid_request_origin? && any_authenticity_token_valid?)
#
# with the candidate tokens being the `authenticity_token` param and
# the `X-CSRF-Token` header — the header is how campfire's JavaScript
# posts (its uploader, and every Turbo fetch reading the page's meta
# tag), and how Rails' own token-less cached boost forms submit at all.
#
# Two deliberate differences, both stated:
#
# * TOKENS ARE UNMASKED. Rails hands out a per-render masked token
#   (one-time pad XOR the session token, a BREACH mitigation) and
#   unmasks it here. This runtime's `form_authenticity_token` issues the
#   session token itself, so the comparison is against that verbatim —
#   a token Rails minted never reaches a session this runtime reads.
# * THE ORIGIN CHECK COMPARES HOSTS, not `base_url`. Rails compares the
#   `Origin` header with `request.base_url`, scheme included, and gets
#   the scheme right behind a TLS proxy through `assume_ssl` /
#   `X-Forwarded-Proto`. Neither is modeled here (`base_url` reads only
#   `HTTPS`), so a scheme comparison would reject every POST of a
#   deployment behind TLS termination — campfire's production default.
#   The host (with port) is still compared, which is what stops another
#   site; a missing header passes, and `null` fails, as in Rails.
#
# Ruby family only, like `redirect_back.rb` and for its reasons: it reads
# the parked request, which only the ruby family carries, and it checks
# a token only the ruby family issues (the shared `form_authenticity_token`
# is ""). A method on the shared `runtime/ruby` Base is transpiled to
# every target, and the strict emitters compiled neither its `render`
# nor the dispatcher's call to it. Required from BOTH ruby-family boots,
# after the controller runtime it reopens.
module ActionController
  class Base
    # Rails' `config.action_controller.allow_forgery_protection`, which
    # every generated `config/environments/test.rb` sets to false. The
    # emitted test harness does the same right after it loads boot.rb
    # (the suite runs without RAILS_ENV, so it cannot be read off
    # `Rails.env`); a server leaves it nil, which verifies.
    class << self
      attr_accessor :allow_forgery_protection
    end

    def verify_authenticity_token
      unless verified_request?
        render "<h1>422 Unprocessable Content</h1>", status: :unprocessable_content
      end
      nil
    end

    def verified_request?
      return true if Base.allow_forgery_protection == false
      req = ActionController::Current.request
      return false if req.nil?
      verb = req.request_method
      return true if verb == "GET" || verb == "HEAD"
      return false unless RequestForgeryProtection.valid_origin?(
        req.env.fetch("HTTP_ORIGIN", "").to_s, req.host.to_s)
      expected = session[:_csrf_token].to_s
      return true if RequestForgeryProtection.token_matches?(
        Params.str(params, "authenticity_token", ""), expected)
      RequestForgeryProtection.token_matches?(
        req.env.fetch("HTTP_X_CSRF_TOKEN", "").to_s, expected)
    end
  end

  module RequestForgeryProtection
    # An absent Origin passes (some agents omit it); `null` — a sandboxed
    # frame, a privacy redirect — does not. Otherwise the header's host
    # must be the request's own Host.
    def self.valid_origin?(origin, host)
      return true if origin.empty?
      return false if origin == "null"
      at = origin.index("://")
      return false if at.nil?
      origin[at + 3, origin.length].to_s == host
    end

    # Constant-time over equal lengths, as ActiveSupport's
    # `secure_compare`: the loop never exits early on a mismatch, so the
    # time taken does not say how many leading bytes were right. An
    # empty expected token — a session no form was ever rendered for —
    # matches nothing.
    def self.token_matches?(given, expected)
      return false if expected.empty?
      return false if given.bytesize != expected.bytesize
      diff = 0
      i = 0
      while i < given.bytesize
        diff = diff | (given.getbyte(i) ^ expected.getbyte(i))
        i += 1
      end
      diff == 0
    end
  end
end
