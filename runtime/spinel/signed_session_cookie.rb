# The session cookie, signed. The dispatchers restore the session from
# `from_signed_cookie` and write it back through `signed_cookie`, so a
# client can read its session but not forge one: an edited value, a
# value signed under another secret, or one minted for another cookie
# name fails the HMAC and reads as an EMPTY session — what Rails does
# with a session cookie it cannot authenticate.
#
# The signature is the one `cookies.signed` already writes
# (`ActionController::MessageVerifier`: PBKDF2 key from
# SECRET_KEY_BASE, HMAC-SHA1, the `_rails` envelope with purpose
# `cookie.<name>`), so there is one signing path to trust rather than two.
#
# NOT Rails' session cookie, in two ways, both stated:
#
# * SIGNED, NOT ENCRYPTED. Rails' cookie store encrypts (AES-256-GCM,
#   the "authenticated encrypted cookie" salt), so a client cannot read
#   its own session either. Here it can: the payload is base64 of the
#   url-encoded `k=v` pairs. The session holds the CSRF token (already
#   in every page it renders) and app keys such as campfire's
#   `return_to_after_authenticating` — integrity is what the CSRF check
#   and the app need; confidentiality is the gap.
# * NOT INTEROPERABLE. The payload is this runtime's `k=v` encoding, not
#   Rails' JSON, so a Rails session cookie does not restore here (it
#   reads as empty) and ours would not restore in Rails. A migration
#   from Rails signs every user out of the SESSION — campfire's login
#   rides its own `cookies.signed[:session_token]`, which does carry
#   over.
#
# Ruby family only, like `request_forgery_protection.rb` beside it: the
# session cookie is written only by the two ruby-family dispatchers.
# Required from BOTH boots, after the controller runtime.
module ActionDispatch
  class Session
    def self.from_signed_cookie(raw, name)
      return Session.from_cookie("") if raw == ""
      Session.from_cookie(
        ActionController::MessageVerifier.verified(
          Rails.application.secret_key_base,
          ActionController::MessageVerifier::SIGNED_COOKIE_SALT,
          raw, "cookie." + name, true
        )
      )
    end

    # `plain` is `Session#to_cookie`'s encoding. The dispatchers compare
    # plain encodings to decide whether the session changed, and sign
    # only what they write.
    def self.signed_cookie(plain, name)
      ActionController::MessageVerifier.generate(
        Rails.application.secret_key_base,
        ActionController::MessageVerifier::SIGNED_COOKIE_SALT,
        plain, "cookie." + name, true
      )
    end
  end
end
