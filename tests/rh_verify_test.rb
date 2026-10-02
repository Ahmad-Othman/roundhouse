require 'minitest/autorun'
require 'tmpdir'
require 'fileutils'
require 'open3'
require 'json'
require 'rbconfig'

class RhVerifyTest < Minitest::Test
  SOURCE = File.expand_path('..', __dir__)

  def setup
    @root = Dir.mktmpdir('rh-verify-')
    %w[bin scripts tests src/emit docs/guide fixtures/real-blog fixtures/store mocks].each do |dir|
      FileUtils.mkdir_p(File.join(@root, dir))
    end
    FileUtils.cp(File.join(SOURCE, 'bin/rh'), File.join(@root, 'bin/rh'))
    FileUtils.cp(File.join(SOURCE, 'scripts/ci-plan.py'), File.join(@root, 'scripts/ci-plan.py'))
    File.write(File.join(@root, '.gitignore'), "mocks/\ncargo.log\n")
    File.write(File.join(@root, 'src/emit/go.rs'), 'before')
    %w[example ruby_toolchain].each { |name| File.write(File.join(@root, "tests/#{name}.rs"), '') }
    git('init', '-q')
    git('add', '.')
    git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'base')
    @base = git('rev-parse', 'HEAD').strip
    @log = File.join(@root, 'cargo.log')
    cargo = File.join(@root, 'mocks/cargo')
    File.write(cargo, <<~RUBY)
      #!#{RbConfig.ruby}
      require 'json'
      if ARGV == ['--version']
        puts 'cargo 1.98.1'
        exit 0
      end
      File.open(ENV.fetch('VERIFY_LOG'), 'a') do |f|
        f.puts JSON.generate(args: ARGV, cwd: Dir.pwd, jobs: ENV['CARGO_BUILD_JOBS'], debug: ENV['CARGO_PROFILE_TEST_DEBUG'])
      end
      puts 'child stdout'
      warn 'child stderr'
      exit(ARGV.include?(ENV['VERIFY_FAIL']) ? 17 : 0)
    RUBY
    File.chmod(0o755, cargo)
  end

  def teardown
    FileUtils.remove_entry(@root)
  end

  def git(*args)
    out, err, status = Open3.capture3('git', *args, chdir: @root)
    assert status.success?, err
    out
  end

  def invoke(*args, env: {})
    Open3.capture3({ 'PATH' => "#{@root}/mocks:#{ENV.fetch('PATH')}", 'VERIFY_LOG' => @log,
      'CARGO_BUILD_JOBS' => nil, 'CARGO_PROFILE_TEST_DEBUG' => nil }.merge(env),
      RbConfig.ruby, File.join(@root, 'bin/rh'), 'verify', *args, chdir: '/')
  end

  def calls
    File.exist?(@log) ? File.readlines(@log).map { |line| JSON.parse(line) } : []
  end

  def test_plan_is_read_only_and_routes_committed_dirty_deleted_and_untracked_inputs
    File.write(File.join(@root, 'docs/guide/verify.md'), 'committed guide')
    git('add', 'docs')
    git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'guide')
    File.delete(File.join(@root, 'src/emit/go.rs'))
    FileUtils.mkdir_p(File.join(@root, 'wasm'))
    File.write(File.join(@root, 'wasm/new file.txt'), 'untracked')
    out, err, status = invoke('--plan', '--json', '--base', @base, '--test', 'example')
    assert status.success?, err
    report = JSON.parse(out)
    assert_equal 'planned', report['status']
    assert_equal ['docs/guide/verify.md', 'src/emit/go.rs', 'wasm/new file.txt'], report['changes']
    assert_includes report['hosted_coverage']['smoke'], 'go'
    assert report['hosted_coverage']['wasm']
    assert_equal false, report['hosted_coverage']['publish']
    assert_equal %w[not-run not-run not-run], report['checks'].map { |c| c['status'] }
    assert_empty calls
    refute File.exist?(File.join(@root, '.git/rh-verify.lock'))
    refute File.exist?(File.join(@root, 'scripts/__pycache__'))
  end

  def test_success_executes_exact_commands_in_order_and_keeps_json_clean
    out, err, status = invoke('--json', '--jobs', '2', '--test', 'example', '--test', 'example', '--toolchain', 'ruby')
    assert status.success?, err
    report = JSON.parse(out)
    assert_equal 'passed', report['status']
    assert_equal [
      %w[build --locked --tests], %w[test --locked --lib -- --test-threads=1],
      %w[test --locked --test example -- --test-threads=1],
      %w[test --locked --test ruby_toolchain -- --ignored --test-threads=1]
    ], calls.map { |c| c['args'] }
    assert calls.all? { |c| c['cwd'] == @root && c['jobs'] == '2' && c['debug'] == '0' }
    assert report['checks'].all? { |c| c['status'] == 'passed' && c['exit'] == 0 && c['seconds'] >= 0 }
    assert_includes err, 'child stdout'
    assert_includes report['scope'], 'not executed'
  end

  def test_failure_stops_execution_preserves_exit_and_leaves_remaining_check_unrun
    out, _err, status = invoke('--json', '--test', 'example', env: { 'VERIFY_FAIL' => '--lib' })
    assert_equal 17, status.exitstatus
    report = JSON.parse(out)
    assert_equal 'failed', report['status']
    assert_equal %w[passed failed not-run], report['checks'].map { |c| c['status'] }
    assert_equal 2, calls.length
  end

  def test_explicit_environment_is_respected
    out, err, status = invoke('--json', env: { 'CARGO_BUILD_JOBS' => '3', 'CARGO_PROFILE_TEST_DEBUG' => '1' })
    assert status.success?, err
    assert_equal '3', JSON.parse(out)['build_environment']['CARGO_BUILD_JOBS']
    assert calls.all? { |c| c['jobs'] == '3' && c['debug'] == '1' }
  end

  def test_missing_fixtures_are_reported_without_blocking_unrelated_checks
    FileUtils.remove_entry(File.join(@root, 'fixtures/store'))
    out, err, status = invoke('--json')
    assert status.success?, err
    assert_equal 'passed', JSON.parse(out)['status']
    assert_equal ['store'], JSON.parse(out)['missing_fixtures']
    assert_equal 2, calls.length
    out, err, status = invoke('--plan', '--json')
    assert status.success?, err
    assert_equal 'planned', JSON.parse(out)['status']
    assert_equal 2, calls.length
  end

  def test_informational_policy_failure_does_not_block_local_execution
    File.write(File.join(@root, 'scripts/ci-plan.py'), "raise RuntimeError('unavailable policy')\n")
    out, err, status = invoke('--json')
    assert status.success?, err
    report = JSON.parse(out)
    assert_equal 'passed', report['status']
    assert_nil report['hosted_coverage']
    assert_includes report['hosted_coverage_error'], 'unavailable policy'
    assert_equal 2, calls.length
  end

  def test_changed_policy_format_remains_informational
    File.write(File.join(@root, 'scripts/ci-plan.py'), "def select(paths): return []\n")
    out, err, status = invoke
    assert status.success?, err
    assert_includes out, 'Hosted selection unavailable: unsupported hosted coverage format'
    assert_includes out, 'Local checks: passed'
    assert_equal 2, calls.length
  end

  def test_help_human_results_and_doctor_discover_verification
    out, err, status = invoke('--help')
    assert status.success?, err
    assert_includes out, '--plan'
    assert_empty calls
    out, err, status = invoke
    assert status.success?, err
    assert_includes out, 'Local checks: passed'
    assert_includes out, 'not hosted CI'
    out, _err, status = invoke(env: { 'VERIFY_FAIL' => '--lib' })
    assert_equal 17, status.exitstatus
    assert_includes out, 'Local checks: failed'
    out, err, status = Open3.capture3({ 'PATH' => "#{@root}/mocks:#{ENV.fetch('PATH')}", 'VERIFY_LOG' => @log },
      RbConfig.ruby, File.join(@root, 'bin/rh'), 'doctor')
    assert status.success?, err
    assert_includes out, 'bin/rh verify --plan'
    assert_includes out, 'bin/rh verify'
    assert_includes out, 'optional verify hosted-coverage preview'
  end

  def test_invalid_inputs_never_start_cargo
    [%w[--test ../example], %w[--toolchain unknown], %w[--jobs 0],
     %w[--base nonexistent], %w[--base --help], %w[extra]].each do |args|
      _out, err, status = invoke(*args)
      assert_equal 2, status.exitstatus, args.inspect
      assert_includes err, 'rh verify:'
    end
    assert_empty calls
  end

  def test_concurrent_run_is_rejected
    File.open(File.join(@root, '.git/rh-verify.lock'), 'w') do |lock|
      lock.flock(File::LOCK_EX)
      _out, err, status = invoke('--json')
      assert_equal 2, status.exitstatus
      assert_includes err, 'another verify run'
      assert_empty calls
    end
  end
end
