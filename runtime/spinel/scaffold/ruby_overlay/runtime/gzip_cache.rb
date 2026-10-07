# frozen_string_literal: true

# Cache of gzip(identity body) for the CRuby overlay.
#
# Rack::Deflater compresses every response. A campfire room page is the
# same ~420 KB HTML for every wrk GET that shares a session, so that is
# the same deflate over and over.
#
# Hit path, measured on 420 KB identity HTML:
#   * `join_body` of Rack `[body]` must not `join` (that copied 420 KB).
#   * Last-identity compare (`bytesize` then `==`) is ~7 µs; MRI string
#     hash of a fresh 420 KB body is ~294 µs. wrk hammers one URL, so
#     the last-hit wins.
#   * The Hash fallback keys by the body's String#hash, its size and its
#     CRC-32 — not the identity bytes (holding 64 × 420 KB strings as Hash
#     keys was the RSS cost). A wrong-body hit needs a 64-bit hash, the
#     size and a CRC-32 to collide at once. It used to be SHA-256, which
#     at 3.5 ms per 460 KB (2 GHz) was a quarter of a request that missed
#     — and a page with a per-request token always misses. Both String#hash
#     and Zlib.crc32 are C and run once each per miss.
#
# Gzip itself runs outside the lock. HTML only, same skips as tep.
#
# SPLICING. A page that varies per request (a fresh masked CSRF token in
# the layout) misses both caches above, and then paid for the whole page:
# on campfire's ~460 KB room page, SHA-256 (3.5 ms) plus level-6 gzip
# (6.2 ms) of 13.3 ms per request at 2 GHz. Most of that page is ONE
# cached fragment (the message collection, ~400 KB) that is the same frozen
# String on every request until a message changes. So, after the once-
# campfire Ruby port's page_parts.rb (itself after the Rust port's
# deflater/splice.rs):
#   * the fragment cache notes each large fragment it hands out during the
#     request (Recorder, prepended onto the overlay's Rails::MemoryStore);
#   * each fragment is deflated ONCE, raw, with no preset dictionary so the
#     piece is valid wherever it lands, SYNC_FLUSHed to a byte boundary and
#     kept with its CRC-32 in a WeakKeyMap (keys are not held alive: a
#     replaced fragment's piece goes with it);
#   * per request only the text between fragments is deflated, at
#     BEST_SPEED, with the real preceding 32 KB as its dictionary;
#   * the pieces are framed as one gzip member: header, pieces, a final
#     empty block, CRC-32 (crc32_combine) and length.
# Fragments are FOUND in the final body (byteindex, in recording order)
# rather than tracked by offset: a view renders into its own buffer and
# the layout appends that buffer, so an offset taken while rendering is
# not an offset in the body. A fragment nested inside a later one (a
# collection miss writes its members, then the collection) is found in
# order and the container skipped. No fragment found: the paths above.
require "zlib"

module GzipCache
  MAX_ENTRIES = 64

  @store = {}
  @mutex = Mutex.new
  @last_raw = nil
  @last_gz = nil

  def self.wrap(app)
    lambda { |env| call(app, env) }
  end

  def self.call(app, env)
    prev = Thread.current[:rh_gzip_fragments]
    Thread.current[:rh_gzip_fragments] = []
    begin
      status, headers, body = app.call(env)
      fragments = Thread.current[:rh_gzip_fragments]
    ensure
      Thread.current[:rh_gzip_fragments] = prev
    end
    maybe_gzip(env, status, headers, body, fragments)
  end

  def self.maybe_gzip(env, status, headers, body, fragments = nil)
    return [status, headers, body] if status < 200 || status == 204 || status == 304
    return [status, headers, body] if env["REQUEST_METHOD"] == "HEAD"
    accept = env["HTTP_ACCEPT_ENCODING"].to_s
    return [status, headers, body] unless accepts_gzip?(accept)
    return [status, headers, body] if header(headers, "content-encoding")
    raw = join_body(body)
    return [status, headers, [raw]] if raw.bytesize < 64
    return [status, headers, [raw]] if binary?(header(headers, "content-type"))
    gz = compress(raw, fragments)
    headers = headers.dup
    headers["content-encoding"] = "gzip"
    vary = header(headers, "vary")
    if vary.nil? || vary.empty?
      headers["vary"] = "Accept-Encoding"
    elsif !vary.downcase.include?("accept-encoding")
      headers["vary"] = "#{vary}, Accept-Encoding"
    end
    headers["content-length"] = gz.bytesize.to_s
    [status, headers, [gz]]
  end

  def self.compress(raw, fragments = nil)
    @mutex.synchronize do
      lr = @last_raw
      if !lr.nil? && lr.bytesize == raw.bytesize && lr == raw
        return @last_gz
      end
    end
    if !fragments.nil? && !fragments.empty?
      spliced = splice(raw, fragments)
      return spliced unless spliced.nil?
    end
    dig = [raw.hash, raw.bytesize, Zlib.crc32(raw)]
    hit = nil
    @mutex.synchronize do
      hit = @store[dig]
    end
    return hit unless hit.nil?
    gz = Zlib.gzip(raw)
    @mutex.synchronize do
      if @store.size >= MAX_ENTRIES
        @store.clear
      end
      @store[dig] = gz
      # Snapshot: the Rack body string can be reused and mutated
      # between requests. Sharing it would make last-hit `==` match
      # the mutated bytes while `@last_gz` is still the old gzip.
      @last_raw = raw.dup
      @last_gz = gz
    end
    gz
  end

  # ── Splicing ──

  # Fragments smaller than this are left in the per-request text: a piece
  # costs a WeakKeyMap lookup (one hash of the fragment) and a flush.
  SPLICE_MIN = 1024
  # Deflate's window, and so the most preset dictionary that can matter.
  WINDOW = 32 * 1024
  # Text pieces shorter than this go out as stored (uncompressed) deflate
  # blocks. Between consecutive fragments (search results, a message list)
  # the text is a few bytes of glue, and a Deflate per piece spent its time
  # in set_dictionary on 32 KB it would barely use: 26% of the search page.
  STORE_MAX = 1024
  # gzip member header: deflate, no flags, mtime 0, no extra flags, Unix.
  GZIP_HEADER = "\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03".b.freeze
  # A final, empty, fixed-Huffman block: ends the deflate stream after
  # pieces that each end on a non-final SYNC_FLUSH block.
  FINAL_BLOCK = "\x03\x00".b.freeze
  SPLICE_OK = Zlib.respond_to?(:crc32_combine) && defined?(ObjectSpace::WeakKeyMap) ? true : false

  @pieces = SPLICE_OK ? ObjectSpace::WeakKeyMap.new : nil
  @pieces_mutex = Mutex.new
  # The last splice: [fragments, texts, gzip]. A page that comes back with
  # the same text around the same fragments (one with no per-request token)
  # reuses it without deflating anything.
  @last_splice = nil
  # How much of a fragment's head is searched for; the rest is confirmed
  # with one comparison. byteindex of a whole ~400 KB fragment was the top
  # frame of a spliced request.
  PROBE_CHARS = 64

  # Called by the cache store for each fragment it hands to a view.
  def self.note_fragment(fragment)
    list = Thread.current[:rh_gzip_fragments]
    return if list.nil? || !fragment.is_a?(String) || !fragment.frozen?
    return if fragment.bytesize < SPLICE_MIN
    list << fragment
  end

  # One gzip member for `raw`, or nil when no recorded fragment is in it.
  def self.splice(raw, fragments)
    return nil unless SPLICE_OK
    found = []
    pos = 0
    fragments.each do |frag|
      at = locate(raw, frag, pos)
      next if at.nil?
      found << at << frag
      pos = at + frag.bytesize
    end
    return nil if found.empty?

    # The text between fragments, as slices of `raw` (shared, not copied).
    texts = []
    frags = []
    cur = 0
    i = 0
    while i < found.length
      texts << raw.byteslice(cur, found[i] - cur)
      frags << found[i + 1]
      cur = found[i] + found[i + 1].bytesize
      i += 2
    end
    texts << raw.byteslice(cur, raw.bytesize - cur)

    last = @pieces_mutex.synchronize { @last_splice }
    if !last.nil? && same_splice?(last, frags, texts)
      # A repeated page: hand it to the last-hit path in `compress`, so the
      # next repeat is one comparison instead of a locate. Only a repeat
      # pays for this copy; a page with a per-request token never does.
      @mutex.synchronize do
        @last_raw = raw.dup
        @last_gz = last[2]
      end
      return last[2]
    end

    out = String.new(capacity: raw.bytesize / 6, encoding: Encoding::BINARY)
    out << GZIP_HEADER
    crc = 0
    pos = 0
    texts.each_with_index do |text, k|
      crc = splice_text(out, raw, pos, text, crc) unless text.empty?
      pos += text.bytesize
      frag = frags[k]
      next if frag.nil?
      piece, piece_crc = fragment_piece(frag)
      out << piece
      crc = Zlib.crc32_combine(crc, piece_crc, frag.bytesize)
      pos += frag.bytesize
    end
    out << FINAL_BLOCK
    out << [crc, raw.bytesize & 0xffffffff].pack("VV")
    out.freeze
    # Texts are copied for keeping: the Rack body string may be reused and
    # mutated between requests (see compress), and they are small.
    @pieces_mutex.synchronize { @last_splice = [frags, texts.map { |t| t.dup.freeze }, out].freeze }
    out
  end

  # Where `frag` starts in `raw` at or after byte `pos`, or nil: its first
  # PROBE_CHARS characters are searched for, then the whole is compared.
  def self.locate(raw, frag, pos)
    probe = frag[0, PROBE_CHARS]
    at = raw.byteindex(probe, pos)
    until at.nil?
      return at if raw.byteslice(at, frag.bytesize) == frag
      at = raw.byteindex(probe, at + probe.bytesize)
    end
    nil
  rescue ArgumentError, IndexError, Encoding::CompatibilityError
    nil
  end

  def self.same_splice?(last, frags, texts)
    lf, lt = last
    return false unless lf.length == frags.length && lt.length == texts.length
    frags.each_with_index { |f, k| return false unless lf[k].equal?(f) }
    texts.each_with_index { |t, k| return false unless lt[k].bytesize == t.bytesize && lt[k] == t }
    true
  end

  # The per-request text `data`, starting at byte `pos`: fastest level, with the
  # body's real preceding bytes (up to the window) as its dictionary.
  def self.splice_text(out, raw, pos, data, crc)
    if data.bytesize < STORE_MAX
      # A stored block: BFINAL 0, BTYPE 00, padded to the byte boundary the
      # previous piece's SYNC_FLUSH left, then LEN, ~LEN and the bytes.
      n = data.bytesize
      out << [0, n, n ^ 0xffff].pack("Cvv") << data.b
    else
      start = pos > WINDOW ? pos - WINDOW : 0
      dict = pos.zero? ? nil : raw.byteslice(start, pos - start)
      out << raw_deflate(data, dict, Zlib::BEST_SPEED)
    end
    Zlib.crc32_combine(crc, Zlib.crc32(data), data.bytesize)
  end

  # A fragment's piece and CRC-32, deflated the first time it is seen.
  def self.fragment_piece(frag)
    hit = @pieces_mutex.synchronize { @pieces[frag] }
    return hit unless hit.nil?
    entry = [raw_deflate(frag, nil, Zlib::DEFAULT_COMPRESSION).freeze, Zlib.crc32(frag)].freeze
    @pieces_mutex.synchronize { @pieces[frag] = entry }
    entry
  end

  # Raw deflate (no zlib or gzip framing), ending byte-aligned on a
  # non-final block, so pieces concatenate into one stream.
  def self.raw_deflate(data, dict, level)
    z = Zlib::Deflate.new(level, -Zlib::MAX_WBITS)
    z.set_dictionary(dict) unless dict.nil?
    s = z.deflate(data, Zlib::SYNC_FLUSH)
    z.close
    s
  end

  # Prepended onto the overlay's cache store: every fragment a view gets
  # from `read_str` (a hit) or `write_str` (a miss, which returns the
  # stored frozen copy the view then appends) is noted for the splice.
  module Recorder
    def read_str(key)
      s = super
      GzipCache.note_fragment(s) unless s.nil?
      s
    end

    def write_str(key, value, ttl)
      s = super
      GzipCache.note_fragment(s)
      s
    end
  end

  def self.join_body(body)
    if body.is_a?(Array)
      n = body.length
      if n == 0
        body.close if body.respond_to?(:close)
        return ""
      end
      if n == 1
        s = body[0].to_s
        body.close if body.respond_to?(:close)
        return s
      end
    end
    parts = []
    body.each { |part| parts << part.to_s }
    body.close if body.respond_to?(:close)
    parts.join
  end

  def self.header(headers, name)
    headers[name] || headers[name.split("-").map(&:capitalize).join("-")]
  end

  def self.accepts_gzip?(accept)
    accept.to_s.downcase.split(",").any? { |part|
      coding, *params = part.strip.split(";")
      next false unless coding == "gzip" || coding == "x-gzip"
      q = "1"
      params.each { |p|
        k, v = p.strip.split("=", 2)
        q = v.to_s if k == "q"
      }
      q.to_f > 0.0
    }
  end

  def self.binary?(ct)
    return false if ct.nil? || ct.empty?
    ct.start_with?("image/") || ct.start_with?("audio/") ||
      ct.start_with?("video/") || ct.start_with?("font/") ||
      ct.start_with?("application/octet-stream") ||
      ct.start_with?("application/zip") ||
      ct.start_with?("application/gzip") ||
      ct.start_with?("application/wasm")
  end
end

Rails::MemoryStore.prepend(GzipCache::Recorder) if defined?(Rails::MemoryStore)
