@decision
Feature: deadlock_watchdog_gate
  Port of the ole-eval vLLM deadlock watchdog (code/inference/fixes/
  vllm_deadlock_watchdog_20260827/vllm_watchdog.py, `VLLMWatchdog.run_once` + `is_stuck`):
  a supervisor that, once a tick, decides whether a vLLM server that still answers
  /health but makes no progress is deadlocked -- and if so, restarts its container.
  It is the gate that stands between "the server looks up" and "the server is doing
  anything", so it names its verdict rather than reporting a number.

  It makes three calls, in the source's own order:

    - health: if /health does not answer, the container may be down and the tick
      does nothing at all -- it neither measures nor restarts (the watchdog waits
      for the supervisor to bring it back).
    - progress: for a server that is up, is it making progress? `is_stuck` returns
      not-stuck when running requests == 0 (idle), or generation throughput > 0
      (normal decode), or prompt throughput > 0 (prefill, which can run long); it is
      stuck only when running > 0 and both throughputs are 0 -- a server holding
      requests while moving nothing.
    - dwell: a stuck server is not restarted on sight. The first stuck tick only arms
      the accumulator (`stuck_since`); a later tick whose stuck duration has reached
      `stuck_threshold` (default 120s) restarts the container and clears the
      accumulator. In between, it keeps waiting.

  The walk encodes exactly that: health -> progress -> dwell. The first call that
  resolves the tick without a restart ends it and names itself; only a stuck server
  whose dwell has reached the threshold reaches RESTART_CONTAINER.

  Two folds are documented:

    - the accumulator is state *across* ticks. `stuck_since` is carried between
      invocations, so the graph models one tick given the bucket that accumulator
      implies (first / under / over) -- that bucket is the graph's observable input.
    - the upstream is a daemon loop (`run_forever`); the graph is one tick, the unit
      whose return value (True iff it restarted) is the verdict.

  Everything numeric stays in the harness: the three throughputs, the running count
  and the dwell duration are computed there, and the graph reads none of them, only
  the verdicts (health up/down, the progress classification, the dwell bucket).

  One stale upstream bug is recorded, and the port follows the *policy the code
  intends*: `parse_engine_state` currently returns None for every real `Engine 000:`
  line (its index reads `float(parts[i+3])`, i.e. `float("tokens/s,")`), and the
  metrics fallback builds `VLLMState` without `prompt_throughput` and raises
  TypeError -- so the shipped watchdog raises on every tick and never restarts
  anything. The graph ports the decision `is_stuck` and `run_once` describe, not the
  broken plumbing that starves them; the harness feeds the graph the verdicts.

  Reference walk (vllm_watchdog.py, run_once): health first; then is_stuck (running==0
  -> no, gen>0 -> no, prompt>0 -> no, else yes); for a stuck state, first tick arms,
  and a tick whose stuck duration is >= stuck_threshold restarts.

  Background:
    Given the workflow runs at most 6 iterations

  Scenario: health
    Given a choice question "tick" asks "Does the server answer /health? If it does not, the container may be down: this tick does nothing at all, neither measuring nor restarting."
      | answer | meaning |
      | up | /health answered |
      | down | /health did not answer -- the container may be down |
    And the answer comes from the field "ev_health"
    When the answer "down" goes to "STOP"
    And any other answer goes to "progress"
    Then the primary question is "tick"
    Then label "NO_ACTION_UNHEALTHY" when the answer to "tick" is "down"
    And label "HEALTH_OK" otherwise

  Scenario: progress
    Given a choice question "tick" asks "Is the running server making progress? It is idle when no request is running, decoding when generation throughput is positive, prefill when prompt throughput is positive, and stuck only when requests are running while both throughputs are zero. Anything but stuck clears the stuck accumulator and ends the tick."
      | answer | meaning |
      | idle | no request is running (running == 0) -- not stuck |
      | decoding | generation throughput is positive -- normal decode |
      | prefill | prompt throughput is positive -- prefill, which can run long |
      | stuck | requests are running but both throughputs are zero |
    And the answer comes from the field "ev_progress"
    When the answer "idle" goes to "STOP"
    And the answer "decoding" goes to "STOP"
    And the answer "prefill" goes to "STOP"
    And any other answer goes to "dwell"
    Then the primary question is "tick"
    Then label "NO_ACTION_IDLE" when the answer to "tick" is "idle"
    And label "NO_ACTION_DECODING" when the answer to "tick" is "decoding"
    And label "NO_ACTION_PREFILL" when the answer to "tick" is "prefill"
    And label "PROGRESS_STUCK" otherwise

  Scenario: dwell
    Given a choice question "tick" asks "The server is stuck. How long has it been stuck? The first stuck tick only arms the accumulator; a later tick whose stuck duration has reached the threshold restarts the container. This is the last call: reaching the threshold restarts."
      | answer | meaning |
      | first | this is the first stuck tick -- nothing had been armed before |
      | under | it was already stuck, and the stuck duration is still below the threshold |
      | over | it was already stuck, and the stuck duration has reached the threshold |
    And the answer comes from the field "ev_dwell"
    When any other answer goes to "STOP"
    Then the primary question is "tick"
    Then label "STUCK_ARMED" when the answer to "tick" is "first"
    And label "STUCK_WAITING" when the answer to "tick" is "under"
    And label "RESTART_CONTAINER" otherwise
