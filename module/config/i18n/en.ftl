# en.ftl: [main-monitor] [app-detect] [screen-detect] [monitors] [scheduler] [scheduler-config-watcher] [sysfs] [clg] [tuned] [touch] [fas] [fas-whitelist] [scheduler-settings] [fast-lock] [logger] [affinity] [corectl] [telemetry] [notify] [config-reload] [governor] [gpu]
# --- Main & Monitor ---
chiri-module-starting = chiri-module Unified Starting
scheduler-module-started = Scheduler module started
scheduler-module-start-failed = Failed to start scheduler module: { $error }
monitor-module-crashed = Monitor module crashed: { $error }
monitor-module-started = Monitor module started
monitor-starting = Starting chiri-monitor module
monitor-screen-watcher-failed = [Main] Screen state watcher thread crashed: { $error }
monitor-config-watcher-failed = [Main] Config watcher thread crashed: { $error }
monitor-fps-crashed = [Main] FPS Monitor crashed: { $error }
monitor-fps-tokio-failed = [Main] Failed to create Tokio runtime for FPS monitor
monitor-cpu-crashed = [Main] CPU Load Monitor crashed: { $error }
monitor-cpu-tokio-failed = [Main] Failed to create Tokio runtime for CPU monitor
monitor-rlimit-memlock-failed = [Main] Failed to raise RLIMIT_MEMLOCK, eBPF maps may fail to load
main-chdir = [Main] Changed working directory to: { $dir }
main-module-root = [Main] Module root: { $path }
main-config-loaded = [Main] Config loaded: { $path } (loglevel={ $loglevel }, language={ $language })
main-module-version = [Main] Module version: { $name } { $version } (versionCode { $code }) | SoC { $soc } | kernel { $kernel }
main-chiri-scheduler-selected = [Main] Supported SoC, Chiri scheduler enabled
main-no-chiri-scheduler = [Main] Unsupported SoC, CPU not taken over
main-diag-thread-started = [Main] Non-ChiRi SoC: independent diagnostic thread started
main-diag-thread-failed = [Main] Failed to start independent diagnostic thread: { $error }
main-diag-thread-panic = [Diag] diag_writer panicked, devimp output stopped
main-special-tuned-exported = [Main] exported { $count } internal special-tuned whitelist entries to special_tuned.yaml
main-log-archive-submitted = [Main] previous logs archived, packing in background to logd/{ $zip }
main-devimp-archive-submitted = [Main] previous devimp diagnostics archived, packing in background to logd/{ $zip }
main-nofix-skip = [Main] meta.yaml has nofix=true: skipping webui asset restore and meta.yaml self-heal
main-webui-restored = [Main] WebUI assets restored from embedded copy: { $count } file(s)
main-webui-not-embedded = [Main] binary has no embedded WebUI assets, restore skipped
main-log-short-session-discarded = [Main] previous session lived less than 30s, logs discarded
monitor-thread-start-screen = [Main] Starting screen state watcher thread
monitor-thread-start-config-watch = [Main] Starting config watcher thread
monitor-thread-start-fps = [Main] Starting eBPF FPS monitor thread
monitor-thread-start-cpu = [Main] Starting eBPF CPU load monitor thread
monitor-thread-start-app-detect = [Main] Starting app detection loop

# --- AppDetect ---
app-detect-config-watch = [AppDetect] watching config file: { $path }
app-detect-change-detected = [AppDetect] change detected, debouncing 100ms
app-detect-reloading = [AppDetect] debounce done, reloading config
app-detect-reload-success = [AppDetect] config reloaded
app-detect-loop-started = [AppDetect] app detection loop started (3000ms poll)
app-detect-screen-changed = [AppDetect] screen state changed: { $old } -> { $new }
app-detect-mode-change-pkg = [AppDetect] mode change: { $old } -> { $new } ({ $pkg })
app-detect-ime-auto = [AppDetect] auto-detected IME: { $pkg }
app-detect-ime-fallback = [AppDetect] IME auto-detect failed, using fallback list
app-detect-debounce-start = [AppDetect] debounce started: new app { $pkg } (pid={ $pid })
app-detect-debounce-confirmed = [AppDetect] debounce confirmed: app { $pkg } (pid={ $pid }) stable
app-detect-pkg-change = [AppDetect] foreground app state: { $pkg } (pid={ $pid }, temp={ $temp }°C, force={ $force })
app-detect-activity-changed = [AppDetect] foreground activity changed: { $pkg } { $activity }
app-detect-no-app = [AppDetect] no valid foreground app detected
app-detect-special-override = [AppDetect] special profile applied: { $pkg } -> { $mode }
app-detect-special-rejected = [AppDetect] non-whitelisted app { $pkg } mapped to special profile { $mode }, rejected, falling back to global mode
app-detect-special-unavailable = [AppDetect] special tuning unavailable, { $pkg } mapped { $mode } not applied, falling back to global mode
app-detect-special-fallback = [AppDetect] special whitelist hit: { $pkg } uses fallback profile { $mode }
app-detect-special-global-rejected = [AppDetect] global mode { $mode } is a special profile and does not apply to non-whitelisted app { $pkg }, falling back to default

# --- ScreenDetect ---
screen-state-change-detected = [Screen] state change detected via '{ $source }'
screen-state-changed-value = [Screen] screen state changed: { $state }
screen-netlink-started = [Screen] netlink socket listener started
screen-state-detect-detail = [Screen] state evaluate: { $old } -> { $new } (src: { $source })
screen-uevent-received = [Screen] uevent received: subsystem={ $subsystem } devpath={ $devpath }
screen-uevent-power-action = [Screen] power action: { $action }
screen-uevent-backlight = [Screen] backlight event: { $dev } -> state={ $state }
screen-uevent-backlight-unreadable = [Screen] backlight state unreadable: { $dev }
screen-detect-source-found = [Screen] screen state source ready: { $kind } @ { $path }
screen-detect-no-source = [Screen] no usable screen state source, uevent only
screen-off-vetoed = [Screen] not enough screen-off votes: node { $node } reports ON (source: { $source }), screen-off not confirmed
screen-off-unconfirmed = [Screen] no readable screen-state node (source: { $source }), screen-off not confirmed
screen-uevent-leds = [Screen] leds backlight event: { $dev } -> state={ $state }
screen-uevent-leds-unreadable = [Screen] leds backlight state unreadable: { $dev }
scheduler-screen-on = [Scheduler] screen ON trigger event
scheduler-screen-off = [Scheduler] screen OFF trigger event

# --- Monitors ---
cpu-monitor-started = [CPU Monitor] eBPF system load monitor started
cpu-monitor-online-cpus-failed = [CPU Monitor] failed to get online CPUs: { $error }
cpu-monitor-online-cpus = [CPU Monitor] detected online CPU core IDs: { $cpus }
cpu-monitor-fg-pid-updated = [CPU Monitor] foreground PID updated { $old } -> { $new }
cpu-monitor-baseline = [CPU Monitor] baseline init | online_cpus={ $cpus } max_cpu_id={ $max_cpu }
cpu-monitor-fg-baseline-reset = [CPU Monitor] foreground PID changed, util baseline reset: { $old } -> { $new }
cpu-monitor-util-fallback = [CPU Monitor] TGID map missing, falling back to thread-level (pid={ $pid }, raw_tgid={ $raw })
cpu-monitor-tick-log = [CPU Monitor] cores=[{ $cores }] fg_pid={ $pid } fg_max_util={ $util }% threads_tracked={ $threads } delta={ $delta }ms
cpu-monitor-channel-closed = [CPU Monitor] channel closed, exiting loop
fps-monitor-init = [FPS Monitor] initializing eBPF FPS monitor
fps-monitor-attached = [FPS Monitor] attached uprobe to PID: { $pid }
fps-monitor-attach-failed-initial = [FPS Monitor] initial attach failed: { $error }
fps-monitor-frame-silent = [FPS Monitor] frame source silent: PID { $pid } probe attached but no frame events for { $secs }s, FAS runs open-loop
fps-monitor-frame-resumed = [FPS Monitor] frame source resumed: PID { $pid } receiving frame events again
fps-monitor-pid-switch-failed = [FPS Monitor] PID switch failed: { $error }
fps-monitor-started = [FPS Monitor] eBPF FPS monitor started (per-PID uprobe)
fps-monitor-passive = [FPS Monitor] FAS not active, probe standing by
fps-monitor-detached = [FPS Monitor] FAS deactivated, probe detached
fps-monitor-symbol-short-miss = [FPS Monitor] short symbol attach failed, trying long symbol
fps-monitor-attach-symbol = [FPS Monitor] attached with symbol: { $lib } (pid={ $pid })
fps-monitor-attach-symbol-name = [FPS Monitor] matched symbol: { $symbol }
fps-monitor-symbol-scan = [FPS Monitor] libgui symbol scan: { $count } queueBuffer variant(s) found
fps-monitor-frame-source-missing = [FPS Monitor] frame source unavailable: no Surface::queueBuffer symbol in { $lib }, FAS gear control stalled, retry in { $secs }s
fps-monitor-frame-summary = [FPS Monitor] frame summary | pid={ $pid } window={ $window } latest={ $latest_ms }ms avg={ $avg_ms }ms
fps-monitor-frames-dropped = [FPS Monitor] event channel congested, { $count } frame samples dropped
fps-monitor-attach-stats = [FPS Monitor] attach failed { $count } time(s) in total, last error: { $error }

# --- Scheduler ---
scheduler-ipc-started = [Scheduler] IPC channel listener started
scheduler-mode-change-request = [Scheduler] mode change request: { $old } -> { $new } (Pkg: { $pkg }, Temp: { $temp })
scheduler-channel-closed = [Scheduler] channel closed, thread exiting
scheduler-ipc-panic = [Scheduler] IPC thread panicked, releasing CPU control
scheduler-ipc-restart = [Scheduler] cleaned up and restarting scheduler loop, attempt { $count }
scheduler-ipc-restart-giveup = [Scheduler] { $count } consecutive panics, giving up on scheduler loop restart
scheduler-doze-enable = [Scheduler] screen OFF: timer started
scheduler-doze-special-keep = [Scheduler] screen OFF: special tuned mode keeps control, skipping CLG
scheduler-doze-restore = [Scheduler] screen ON: restoring previous performance constraints
scheduler-clg-init = [Scheduler] CPU Load Governor: initialized at startup (mode={ $mode })
scheduler-event-screen = [Scheduler] screen event received: on={ $on } (last={ $last })
scheduler-event-mode-change = [Scheduler] mode change event: pkg={ $pkg } { $old } -> { $new } (temp={ $temp })
scheduler-event-load = [Scheduler] load event: core_utils=[{ $cores }]
scheduler-event-frame = [Scheduler] frame event: delta={ $delta_ms }ms
scheduler-event-config-reload = [Scheduler] config reload event: mode={ $mode }, screen_on={ $screen_on }
scheduler-special-mode-active = [Scheduler] special profile active: { $pkg } -> { $mode }
scheduler-tuned-cooldown = [Scheduler] special tuning takeover failed, entering { $secs }s cooldown, CLG takes over
scheduler-scene-mode-enter = [Scheduler] screen off past threshold, switching to scenemode
scheduler-scene-mode-exit-fas = [Scheduler] FAS re-activated, exiting scenemode early
scheduler-scene-mode-exit-switch = [Scheduler] scenemode_enabled disabled, exiting scenemode
scheduler-fas-switch-off = [Scheduler] fas_enabled disabled, deactivating all FAS instances and restoring scheduler takeover
scheduler-scene-mode-saturation = [Scheduler] scenemode perf ceiling saturated (little util { $util }%), falling back to reduce with 300s cooldown

# --- Scheduler: DOWN (halt) ---
scheduler-down-enter = [Scheduler] DOWN halt enabled: CLG/special-tuned/FAS/fast_lock/thread placement/core_ctl all released, collection and logs only
scheduler-down-exit = [Scheduler] DOWN halt lifted, scheduling resumes
down-enabled = [Down] down.chr says down, scheduling halted
down-disabled = [Down] down.chr cleared, scheduling back to normal
down-watch-error = [Down] down.chr watch failed: { $error }
down-boot-halted = [Down] booting in halt mode: no takeover enabled, telemetry and logs continue
down-heartbeat = [Down] halt still in effect ({ $mins } min): scheduling stays released, telemetry and logs continue
down-unsupported-soc = [Down] SoC not in the ChiRi list, external state aligned to halt

# --- Scheduler: Config Watcher ---
config-reloading = [Config] config file change detected, reloading
config-reloaded-success = [Config] config reloaded
config-reload-fail = [Config] config reload failed: { $error }
config-special-parse-failed = [Config] failed to parse special-tuned config: { $path } ({ $error }), whitelisted apps fall back to CLG
config-special-merged = [Config] merged special-tuned config: { $path }

# --- Governor (performance take-over, FAS/contingency) ---
governor-selected = [Governor] default governor pick: { $policies }
governor-switched = [Governor] P{ $pid } governor { $from } -> performance
governor-restored = [Governor] P{ $pid } governor performance -> { $to }
governor-switch-failed = [Governor] failed to switch governor on policy { $pid }
governor-restore-failed = [Governor] failed to restore governor { $governor } on policy { $pid }
governor-residue-cleanup = [Governor] leftover performance governor detected, restored to schedutil
governor-residue-down = [Governor] policy{ $pid } still on performance during halt, not cleaning it up
governor-activate-noop = [Governor] no policy taken over (of { $total }), performance semantics not in effect

# --- GPU frequency lock (contingency) ---
gpu-locked = [GPU] locked to max frequency { $khz } kHz ({ $nodes } nodes)
gpu-released = [GPU] original frequency limits restored
gpu-detect-miss = [GPU] no usable GPU frequency node found, contingency GPU lock skipped
gpu-lock-partial = [GPU] lock succeeded on only { $ok }/{ $total } nodes
gpu-probe-rejected = [GPU] devfreq dir rejected: { $path } ({ $reason })
gpu-snapshot-unavailable = [GPU] pre-takeover snapshot unavailable, original governor/freq may not be restorable
gpu-lock-failed = [GPU] all { $nodes } nodes failed to lock, contingency GPU frequency lock lost
gpu-release-max-stuck = [GPU] { $nodes } nodes failed to restore original max frequency, stuck at max

# --- SysFS generic ---
sysfs-write-failed = [SysFS] failed to write { $path }: { $error }
sysfs-open-failed = [SysFS] failed to open { $path }: { $error }
sysfs-umount2-failed = [SysFS] umount2({ $path }) failed: { $error }
sysfs-write-freq-failed = [SysFS] write freq { $freq } failed: { $error }
config-scenemode-merged = [Config] merged scenemode config: { $path }
config-scenemode-parse-failed = [Config] failed to parse the scenemode config ({ $path }): { $error }, keeping the current scenemode
battery-status-unknown = [Battery] unrecognized status node value: { $raw }, treated as unknown
power-avg-skip = [PowerAVG] this sample was not counted ({ $reason }), PowerAVG.chr keeps its previous value
telemetry-raw-snapshot = [Telemetry] first-reading snapshot: raw voltage={ $v }, raw current={ $i }, divisors voltage/{ $vd }, current/{ $cd } -> V={ $v }/{ $vd }, A={ $i }/{ $cd }
config-watch-error = [Config] failed to watch config directory: { $error }
config-apply-tweaks-failed = [Config] failed to apply reloaded system tweaks: { $error }
config-feature-parse-failed = [Config] feature.yaml parse failed: { $error }
config-meta-fallback = [Config] meta config fell back to embedded defaults: { $path }

# --- CLG ---
clg-init = [CLG] P{ $pid } init | cores={ $cpus } | freqs={ $fmin }-{ $fmax } MHz | P={ $perf } -> { $freq } kHz
clg-activated = [CLG] CPU Load Governor activated, taking over { $count } cluster(s)
powerbase-activated = [PowerBase] power-based governor took over (CLG released)
powerbase-deactivated = [PowerBase] power-based governor released
powerbase-writer-invalid = [PowerBase] P{ $pid } frequency writer unavailable, cluster skipped
powerbase-touch-tier = [PowerBase] touch raise to frontier bucket #{ $tier } ceiling
powerbase-takeover-skipped = [PowerBase] P{ $pid } freq table and soc.yaml fallback both unavailable, cluster takeover skipped
powerbase-init-write-failed = [PowerBase] P{ $pid } initial max write failed, cluster perf stays 0
powerbase-init-min-failed = [PowerBase] P{ $pid } initial min write failed, min cannot be lowered
clg-no-clusters = [CLG] CPU Load Governor: no valid clusters found, staying inactive
clg-deactivated = [CLG] CPU Load Governor deactivated
clg-config-reloaded = [CLG] config hot-reloaded | up={ $up } down={ $down } floor={ $floor } ceil={ $ceil }
clg-perf-clamped = [CLG] config perf_floor > perf_ceil ({ $floor } > { $ceil }), clamped perf_floor to perf_ceil
clg-restore = [CLG] P{ $pid } restored | governor={ $governor } min={ $min } kHz max={ $max } kHz
clg-tick-log = [CLG] P{ $pid } util={ $util }% perf={ $perf } freq={ $freq }kHz boost={ $boost }kHz decay={ $decay }
clg-pf-tick-log = [CLG-PF] P{ $pid } util={ $util }% perf={ $perf } freq={ $freq }kHz decay={ $decay } pf={ $pf_khz }kHz
clg-pf-enabled = [CLG-PF] Pareto frontier lookup enabled | fingerprint={ $fp } buckets={ $buckets }
clg-pf-fallback = [CLG-PF] frontier table unavailable ({ $reason }), falling back to ratio path (once)
clg-writer-invalid = [CLG] P{ $pid } sysfs writer invalid (max_valid: { $max_valid }, min_valid: { $min_valid }), skipping
clg-freq-set = [CLG] P{ $pid } freq change: { $old_khz }MHz -> { $new_khz }MHz
clg-freq-write-failed-cached = [CLG] P{ $pid } freq write failed, keeping cached { $cached_khz }MHz (target { $target_khz }MHz)
clg-watchdog-release = [CLG] WATCHDOG: no load events for { $secs }s, eBPF source failed, releasing CPU control
clg-touch-boost = [CLG] touch boost window open: big-core perf floor={ $floor } held { $ms }ms
clg-thermal-cap = [CLG] thermal guard: battery={ $batt }°C / CPU={ $cpu }°C, perf ceiling capped to { $cap }% (>= { $free } exempt)
clg-thermal-no-sensor = [CLG] thermal guard: no CPU temperature sensor, CPU reference disabled
clg-thermal-no-battery = [CLG] thermal guard: battery temp node not found, CPU-only suppression
clg-min-write-failed = [CLG] P{ $pid } failed to write scaling_min_freq={ $khz }MHz
thermal-config-clamped = [Thermal] thermal config out of range, clamped: { $key } { $value } -> { $fixed }
battery-temp-scale = [Thermal] battery temp scale pre-detected: { $unit } (divisor { $divisor })
battery-temp-scale-unknown = [Thermal] battery temp scale pre-detection inconclusive, degrading to CPU-only this run

# --- Special tuning (per-mode profiles: akmode / playback / daily ...) ---
tuned-init = [Tuned] { $mode } special tuning take over (tier-less load-following)
tuned-activated = [Tuned] { $mode } special tuning activated (schedutil + dynamic max, instant up / debounced down)
tuned-no-clusters = [Tuned] { $mode } special tuning: no valid clusters found, staying inactive
tuned-cluster-skipped = [Tuned] { $mode } P{ $pid } skipped (reason: { $reason })
tuned-deactivated = [Tuned] { $mode } special tuning deactivated
tuned-config-reloaded = [Tuned] { $mode } profile hot-reloaded
tuned-tick-log = [Tuned] { $mode } { $state }
tuned-watchdog-release = [Tuned] WATCHDOG: no load events for { $secs }s, eBPF source failed, releasing special tuning
tuned-profile-missing = [Tuned] whitelist mode { $mode } has no profile, takeover falls back to the akmode section

# --- Touch (touch boost) ---
touch-detect-started = [Touch] touch detection thread started (reading /dev/input)
touch-detect-no-devices = [Touch] no readable input devices found, retrying in 3s
touch-detect-poll-error = [Touch] poll on input devices failed, re-enumerating: { $error }
touch-detect-input-unreadable = [Touch] /dev/input is unreadable, touch detection cannot enumerate devices
touch-detect-device-open-failed = [Touch] { $count } input device(s) failed to open: { $list }
touch-detect-device-error = [Touch] input device { $index } reported an error event (revents={ $revents }), re-enumerating
touch-detect-read-failed = [Touch] input device { $index } read failed or disconnected, re-enumerating
touch-detect-thread-exit = [Touch] touch detection thread exited, touch boost is dead
touch-detect-down = [Touch] touch down detected (type={ $type } code={ $code })
touch-event-received = [Touch] touch event received, boosting big cores and flushing immediately
touch-boost-disable-node = [TouchBoost] wrote { $path } = 0
touch-boost-disable-applied = [TouchBoost] Android built-in touch boost (cpu_boost) disabled
sched-tuning-applied = [Sched] kernel scheduler knobs applied ({ $count } items)
sched-tuning-key-rejected = [Sched] kernel scheduler knob rejected by whitelist: { $key }
system-tweaks-restore = [Tweaks] DOWN halt: one-shot system tweaks restored ({ $count } nodes)
system-tweaks-skipped-down = [Tweaks] one-shot system tweaks skipped during DOWN halt

# --- FAS ---
fas-auto-capacity = [FAS] auto capacity weight:
fas-auto-capacity-core = [FAS]   P{ $pid }: cap={ $cap } -> w={ $weight }
fas-capacity-probe-failed = [FAS] capacity auto-probe failed, falling back to manual weights: probes={ $probes } zero_cap={ $zero }
fas-policy-init = [FAS] P{ $pid } { $min }-{ $max } MHz | w={ $weight }
fas-init-summary = [FAS] init | { $fps }fps margin:{ $margin } clusters:{ $clusters } P:{ $perf } profiles:{ $profiles }
fas-app-switch = [FAS] app switch ({ $ms }ms) | P -> { $perf }
fas-loading-start = [FAS] entering loading state ({ $frames } frames, { $ms }ms) | P { $old_perf } -> { $new_perf }
fas-loading-exit = [FAS] exit loading state | P -> { $perf }
fas-gear-switch = [FAS] gear switch { $old } -> { $new }fps | P -> { $perf }
fas-low-perf-upgrade = [FAS] low-load steady frame upgrade | P={ $perf } avg={ $avg } stddev={ $stddev } -> { $fps }fps
fas-downgrade-boost = [FAS] downgrade boost | avg:{ $avg } | P { $old } -> { $new } (inc={ $inc })
fas-boost-expired = [FAS] boost expired, fast-tracking downgrade (confirm={ $confirm })
fas-floor-rescue = [FAS] floor-rescue | stuck { $frames } frames at P={ $old }, avg:{ $avg } -> P:{ $new }
fas-tick-log = [FAS] { $target }fps avg:{ $avg } | { $ms }ms ema:{ $ema } | err:{ $err_ema }/{ $err_inst } | { $act } | P:{ $perf } fg_util:{ $util }{ $cd }{ $damp }{ $temp }{ $offset }
fas-set-game = [FAS] set_game | pkg={ $pkg } | gears={ $gears } | target={ $target }fps
fas-no-profile = [FAS] no per-app profile for '{ $pkg }', using global gears { $gears }
fas-loop-open = [FAS] frame feedback silent, entering open-loop (load-only), gear adjustments suspended: { $pkg }
fas-loop-close = [FAS] frame feedback restored, back to closed-loop: { $pkg }
fas-pid-reloaded = [FAS] PID coefficients hot-reloaded: Kp={ $kp } Ki={ $ki } Kd={ $kd }
fas-rules-reloaded = [FAS] rules hot-reloaded (margin={ $margin }, floor={ $floor }, ceil={ $ceil }, profiles={ $profiles })
fas-policy-writer-invalid = [FAS] P{ $pid } policy writer invalid (max_valid: { $max_valid }, min_valid: { $min_valid }), skipping
fas-qos-clamp-enter = [FAS] P{ $pid } locked freq clamped by QoS (wrote { $wrote }kHz, read { $read }kHz, dcvsh_limit={ $dcvsh })
fas-qos-clamp-exit = [FAS] P{ $pid } QoS clamp released after { $held }s (max restored to { $freq }kHz)
fas-qos-clamp-long = [FAS] P{ $pid } QoS clamped for { $held }s (wrote { $wrote }kHz, read { $read }kHz)
fas-freq-tamper = [FAS] P{ $pid } locked freq tampered (wrote { $wrote }kHz, read { $read }kHz), re-applying
fas-native-gear-skip-samples = [FAS] native gear detect skipped: { $count }/{ $need } samples, avg:{ $avg }
fas-native-gear-no-match = [FAS] native gear no match: avg:{ $avg } stddev:{ $stddev }, no gear within ±8fps (target { $target }fps)
fas-native-gear-skip-jitter = [FAS] native gear detect skipped: jitter { $stddev } ≥ { $limit }, avg:{ $avg }
fas-native-gear-detected = [FAS] native gear detected: avg:{ $avg } stddev:{ $stddev }, { $from } -> { $to }fps
fas-downgrade-deferred-dwell = [FAS] downgrade deferred by dwell: { $dwell } frames left, avg:{ $avg } below { $threshold } (target { $target }fps)
fas-extreme-jank-fasttrack = [FAS] extreme jank fast-track: avg:{ $avg } far below target { $target }fps (stddev:{ $stddev })
fas-no-policies = [FAS] no cpufreq policy found, FAS takeover failed
fas-governor-write-failed = [FAS] P{ $pid } failed to write performance governor: { $error }
fas-freq-table-unavailable = [FAS] P{ $pid } frequency table unavailable and no soc.yaml fallback ({ $path }), policy skipped
fas-policy-none-taken = [FAS] all policies skipped, no cluster taken over
fas-freq-hold-skip = [FAS] P{ $pid } freq hold { $frames } frames left, write skipped
fas-freq-hysteresis-skip = [FAS] P{ $pid } freq diff { $diff } <= hysteresis { $hysteresis }, write skipped
fas-verify-read-failed = [FAS] P{ $pid } verify read-back failed
fas-verify-read-restored = [FAS] P{ $pid } verify read-back restored, verification chain re-established
fas-boost-ceil-clamped = [FAS] { $act } boost clamped by ceiling: requested { $req } -> P { $perf } (ceiling { $ceil })
fas-boost-max-inc-clamped = [FAS] { $act } boost limited by max step: requested { $req } -> P { $perf } (max inc { $max_inc })
fas-boost-damped-cap-clamped = [FAS] { $act } boost clamped by damped cap: requested { $req } -> P { $cap }
fas-frame-dropped = [FAS] frame sample dropped: { $ms }ms ({ $reason })

# --- FAS (whitelist / scheduler integration) ---
main-fas-whitelist-exported = [Main] exported { $count } FAS whitelist entries to fas_whitelist.yaml
app-detect-fas-fallback = [AppDetect] foreground app hit FAS whitelist, entering FAS mode: { $pkg }
app-detect-pkg-normalized = [AppDetect] foreground name has a sub-process suffix, normalized to the base package: { $pkg } -> { $base }
rule-key-normalized = [Rules] rule key has a sub-process suffix, treated as its base package: { $key } -> { $base }
rule-key-conflict = [Rules] normalized rule key conflicts with an existing base-package rule, the latter wins: { $key } -> { $base }
rule-app-modes-empty = [AppDetect] rules app_modes empty or null, treating as empty map
rule-global-mode-defaulted = [AppDetect] rules global_mode missing, defaulted to: { $mode }
app-detect-fas-rejected = [AppDetect] non-whitelisted app { $pkg } mapped to FAS mode { $mode }, rejected, falling back to global mode
app-detect-fas-global-rejected = [AppDetect] global mode { $mode } is a FAS mode and does not apply to non-whitelisted app { $pkg }, falling back to default
scheduler-fas-activate = [Scheduler] FAS instance activated: { $pkg } (pid={ $pid })
scheduler-fas-switch = [Scheduler] FAS instance hot-switched: { $old } -> { $new }
scheduler-fas-deactivate = [Scheduler] FAS instance deactivated, frequencies restored: { $pkg }
scheduler-fas-delayed-exit = [Scheduler] FAS delayed exit expired, governor restored, taking over as { $mode }
scheduler-fas-init-failed = [Scheduler] FAS instance init failed, falling back to CLG: { $pkg }
scheduler-fas-cooldown = [Scheduler] FAS init failed, entering { $secs }s cooldown, CLG takes over

# --- Scheduler: Settings ---
apply-cpu-idle-governor-start = CPU idle governor settings applied
apply-io-settings-start = I/O settings applied
main-config-watch-thread-create = main config watcher thread created

# --- Fast Lock ---
fast-activated = [Fast] activated, all cores locked to max frequency
fast-deactivated = [Fast] deactivated, system frequencies restored
fast-init = [Fast] policy { $pid } locked at { $khz } kHz (target)
fast-rewrite = [Fast] policy { $pid } rewrite { $khz } kHz (target)
fast-writer-invalid = [Fast] policy { $pid } writer invalid (max_valid: { $max_valid }, min_valid: { $min_valid }), skipping
fast-restore = [Fast] policy { $pid } restore governor={ $governor } min={ $min } max={ $max }
fast-watchdog-release = [Fast] load source timeout ({ $secs }s), releasing fast lock
fast-freq-table-unavailable = [Fast] P{ $pid } freq table unavailable with no soc.yaml fallback, cluster not locked: { $path }

# --- Logger ---
log-level-updated = Log level updated to: { $level }
logger-log-restart-for-archive = [Logger] { $dir } reached { $mb }MB, restarting scheduler to archive logs (overrun notification notify={ $notify })
logger-log-restart-suppressed = [Logger] { $dir } reached { $mb }MB, no watchdog detected, auto-restart-for-archive skipped, overrun notification notify={ $notify } alert={ $alert }
logger-log-watchdog-alert-posted = [Logger] no watchdog detected, manual-takeover alert posted (tag ChiRi-Watchdog)
logger-log-restart-fas-deferred = [Logger] { $dir } reached { $mb }MB, current mode { $mode } is FAS, restart and notification skipped, re-evaluated in { $min } minutes
logger-log-restart-mode-unknown = [Logger] cannot read current_mode.chr, proceeding as non-FAS
logger-append-failed = [Logger] log write failed ({ $n } times in a row), handle discarded and rebuilt on the next log
logger-status-write-failed = [Logger] status.csv write failed for the { $n }th consecutive time, that second's row is lost

# --- Rhine (Lab) ---
rhine-state-created = [Rhine] rhine.chr missing, created with default content (not enabled)
rhine-state-invalid = [Rhine] rhine.chr content invalid ({ $value }), reset to default (not enabled)
rhine-state-write-failed = [Rhine] rhine.chr state write-back failed, file and in-memory state diverge
rhine-mode-enabled = [Rhine] lab mode enabled: { $mode }
rhine-mode-disabled = [Rhine] lab mode disabled, changes reverted from backup
rhine-restored = [Rhine] reverted { $origin } changes from rhine-back.chr
rhine-restore-invalid = [Rhine] rhine-back.chr invalid, reverted with built-in defaults and cleared lab overrides
rhine-restore-meta-failed = [Rhine] failed to revert meta.yaml, rhine-back.chr kept for the next attempt
rhine-apply-failed = [Rhine] failed to enable lab mode: { $mode } ({ $error })
rhine-watch-error = [Rhine] rhine.chr watch failed: { $error }
rhine-lock-unavailable = [Rhine] neither /tmp nor /dev is writable, lab lock not created
rhine-lock-refused = [Rhine] lab is locked: turning it off needs a device reboot, request ignored
rhine-lock-lost = [Rhine] lock file exists but the locked mode is unknown, lock cleared
rhine-force-off = [Rhine] rhine.chr says off, lab force-disabled and original values restored
rhine-lock-note-no-backup = rhine-back.chr was missing, rebuilt from built-in defaults

# --- Affinity (CPU affinity & thread migration) ---
affinity-boost-applied = [Affinity] boost layout applied: top-app/foreground → { $big }, background groups → { $little }
affinity-normal-restore = [Affinity] normal affinity layout restored
affinity-promoted = [Affinity] background thread { $tid } promoted to big core (util { $util }%)
affinity-fdp-promoted = [Affinity] FDP: background thread { $tid } migrated to { $dst } (net benefit { $net_mw } mW)
affinity-demoted = [Affinity] background thread { $tid } demoted back to the little cluster (util { $util }%)
affinity-write-fail-summary = [Affinity] affinity write failures in this 25-tick window: mask={ $mask } cpuset={ $cpuset } uclamp={ $uclamp }
affinity-uclamp-unavailable = [Affinity] top_app_uclamp_max_pct unavailable, auto-corrected (kernel { $version }, reason: { $reason })
affinity-released = [Affinity] takeover released, system affinity config restored
affinity-self-root-failed = [Affinity] failed to move daemon's own threads back to original cpuset (errno={ $errno })

# --- CoreCtl (core_ctl online control) ---
corectl-boost-on = [CoreCtl] boost: min_cpus raised to keep all { $count } clusters fully online
corectl-boost-off = [CoreCtl] core_ctl min_cpus snapshot restored
corectl-scenemode-on = [CoreCtl] scenemode core offline: { $count } little cores taken offline
corectl-scenemode-off = [CoreCtl] { $count } offlined cores restored online
corectl-restore-pending = [CoreCtl] { $count } cores failed to come back online, retrying every 2s
corectl-self-pinned = [CoreCtl] scheduler service pinned to dedicated big core cpu{ $core }
corectl-unavailable = [CoreCtl] no usable core_ctl node found, takeover skipped
corectl-write-failed = [CoreCtl] core_ctl write failed: { $path }
corectl-node-missing = [CoreCtl] core_ctl node missing/unreadable: { $path }, cluster falls back to per-core offline
corectl-enable-off = [CoreCtl] core_ctl enable=0, writes have no effect: { $path }, falling back to per-core offline
corectl-enable-off-boost = [CoreCtl] core_ctl enable=0, the whole cluster is already online: { $path }, boost keeps-online skipped
corectl-vendor-override = [CoreCtl] core_ctl node overridden by vendor (read-back differs from target): { $path }
corectl-verify-failed = [CoreCtl] core_ctl node read-back failed, accounted as applied: { $path }
corectl-scenemode-halt = [CoreCtl] scenemode target cluster converged by core_ctl max_cpus
corectl-reserved-core-halted = [CoreCtl] boot core CPU0 went offline after the core_ctl keep value was written, max_cpus rolled back
clampev-node-missing = [Diag] clamp-evidence node unavailable: { $key } ({ $path })

# --- Notify (generic notification component; src/notify.rs hands three args to scripts/notify.sh) ---
# FAS open-loop warning, rate-limited by the ChiRi-FAS interceptor: first 120 silenced + 1h same-text dedup
notify-fas-openloop-title = Warning: FAS open-loop
notify-fas-openloop-body = FAS is not working properly while { $pkg } is running and has entered open-loop mode
notify-post-failed = [Notify] cmd notification post failed (tag={ $tag } title={ $title })
notify-post-ok = [Notify] posted (tag={ $tag } title={ $title } strategy={ $strategy } verify={ $verify })
notify-script-missing = [Notify] notify script missing, notification cannot be posted: { $path }
# Startup-complete notification, main.rs startup tail, tag ChiRi-Status, title/body are literals not i18n
notify-startup-posted = [Notify] startup-complete notification posted (tag ChiRi-Status)
# Remote push, remote_push.rs: fetch remindPush.json every 2h between 08:00 and 24:00, notify only on a versionCode hit
remote-push-started = [Remote] remote-push thread started (fetches every 2 hours between 08:00 and 24:00)
remote-push-primary-failed = [Remote] primary source fetch failed: { $error }, trying the fallback
remote-push-fallback-failed = [Remote] fallback source fetch failed: { $error }, giving up for this slot
remote-push-notified = [Remote] matched versionCode { $code }, reminder notification sent: { $title }
remote-push-no-match = [Remote] no match for versionCode { $code }
remote-push-thread-failed = [Remote] failed to start the remote-push thread: { $error }

# --- Telemetry ---
monitor-thread-start-telemetry = [Main] starting telemetry monitor thread (PSI/GPU/battery)
telemetry-oplus-bcc = [Telemetry] OPlus private node bcc_parms enabled, battery current/voltage reads use real-time BCC data
telemetry-oplus-bcc-missing = [Telemetry] meta enables oplus_chg but bcc_parms does not exist, falling back to the standard power_supply nodes
telemetry-bcc-unusable = [Telemetry] bcc_parms fields 6/8 missing or non-integer, real-time BCC power unavailable, falling back to the standard power_supply node
telemetry-battery-unavailable = [Telemetry] all battery current/voltage candidate nodes unreadable, power/voltage columns written as -
telemetry-gpu-unavailable = [Telemetry] no GPU busy candidate node exists, the GPU column stays -
telemetry-probe-attached = [CPU Monitor] eBPF telemetry probe attached: { $name }
telemetry-probe-failed = [CPU Monitor] eBPF telemetry probe { $name } attach failed: { $error }
telemetry-map-missing = [CPU Monitor] map { $name } missing from eBPF binary, its counters stay 0
telemetry-summary = [Telemetry] PSI cpu={ $cpu }% io={ $io }% mem={ $mem }% | GPU={ $gpu }% | wakeups={ $wakeups } migrations={ $migrations } freq={ $freq } | battery { $power }W

# --- Config hot-reload sync ---
scheduler-config-dirty-reload = [Scheduler] feature.yaml hot reload synced to scheduler (mode={ $mode })

# --- silent-failure coverage ---
app-detect-activity-override = [AppDetect] foreground activity overrides package mapping: { $pkg } <- { $activity }
down-chr-created = [Down] down.chr missing, template recreated (not halted)
down-chr-read-failed = [Down] down.chr read failed, treating as not-halted: { $error }
down-chr-recreate-failed = [Down] down.chr recreate failed, file stays missing
window-probe-query-failed = [WindowProbe] query failed pkg={ $pkg } pid={ $pid }: { $error }
window-probe-stopped = [WindowProbe] probe thread stopped, visibility decisions no longer update
window-probe-worker-failed = [WindowProbe] query worker failed to start/run, window visibility capability lost
activity-sample-failed = [WindowProbe] activity sample failed pkg={ $pkg }: { $error }
activity-sampler-stopped = [WindowProbe] activity sampler thread stopped, activity column no longer updates
activity-sampler-worker-failed = [WindowProbe] activity sampler worker failed to start/run, activity column stays unknown
cpu-monitor-percpu-read-failed = [CPU Monitor] eBPF per-cpu counter read failed, counted as 0 this round
cpu-monitor-tids-read-failed = [CPU Monitor] failed to read thread list of pid { $pid }: { $error }
cpu-monitor-core-state-read-failed = [CPU Monitor] core online state read failed, treating core as offline: { $error }
cpu-monitor-snap-handle-missing = [CPU Monitor] eBPF process-snapshot handle unregistered / map missing, @S frame process section empty
cpu-monitor-snap-map-open-failed = [CPU Monitor] failed to open eBPF process-snapshot map, @S frame process section empty
screen-uevent-recv-failed = [Screen] uevent recv failed, retrying in 1s: { $error }
