# frozen_string_literal: true

# Memoized signed-value verification for the CRuby/JRuby trees.
#
# A browser sends the same signed cookies (the session token, the session
# itself) on every request, and `MessageVerifier.verified_json` re-derives
# the same answer each time: an HMAC over the payload, a base64 decode and
# a scan of the envelope. The answer depends only on its arguments, except
# for an expiry, so it is kept per thread, keyed by the arguments, with the
# envelope's `exp` re-checked against the clock on every hit. Rejections
# ("") are not kept: they are rare, and keeping them would let a flood of
# forged values fill the cache.
#
# Shared runtime/ruby stays as it is for the strict targets and Spinel.
module ActionController
  module MessageVerifier
    VERIFIED_CACHE_CAP = 1024

    class << self
      alias_method :verified_json_uncached, :verified_json

      def verified_json(secret, salt, signed, purpose, sha1)
        cache = (Thread.current[:rh_verified_json] ||= {})
        key = [signed, salt, purpose, sha1, secret]
        hit = cache[key]
        if !hit.nil?
          exp = hit[1]
          return hit[0] if exp == "" || exp > iso8601_ms(Time.now)
          cache.delete(key)
          return ""
        end
        json = verified_json_uncached(secret, salt, signed, purpose, sha1)
        return json if json == ""
        sep = signed.index("--")
        exp = extract(Base64.strict_decode64(signed[0, sep]), "\"exp\":\"")
        cache.clear if cache.size >= VERIFIED_CACHE_CAP
        cache[key] = [json.freeze, exp.freeze].freeze
        json
      end
    end
  end
end
