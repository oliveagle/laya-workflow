@decision
Feature: dev_repo_audit
  Audit one development-repository domain (modelscope, huggingface, github,
  crates, pypi, npm, docker, distro mirrors ...): is it reached by the path its
  category needs, and is that path fast enough? Emits either OK or the concrete
  fix (domestic path, proxy rule, or a domestic mirror suggestion).

  Evidence fields (filled by the inventory harness):
    - ev_domain    the probe target, e.g. "static.crates.io".
    - ev_category  "domestic" (CN service / CN mirror) or "foreign".
    - ev_outbound  the observed outbound from the OpenClash match log:
                   "direct" / "proxy" / "unknown".
    - ev_latency   "fast" (TTFB < 1s), "slow" (1-3s), "timeout" (> 3s/000).
    - ev_mirror    "available" when a measured domestic mirror exists for this
                   service, else "none".

  Expected path by category:
    domestic -> direct + fast           (a domestic service behind the proxy
                                         pays the proxy hop for nothing)
    foreign  -> proxy                   (direct domestic egress is blocked/slow)
  Foreign-but-slow with a domestic mirror is not a routing bug: the fix is to
  point the tool at the mirror (pip/npm/cargo/docker/apt config), hence
  SUGGEST_MIRROR instead of a rule change.

  Emitted labels: OK_DIRECT, OK_PROXY, FIX_DOMESTIC_PATH, FIX_PROXY_RULE,
  SUGGEST_MIRROR, REVIEW.

  Background:
    Given the workflow runs at most 4 iterations

  Scenario: path
    Given a choice question "path_state" asks "Is the observed path the right one for this target's category? expected = domestic+direct or foreign+proxy; domestic_proxied = a domestic target behind the proxy; foreign_direct = a foreign target going direct."
      | answer           | meaning |
      | expected         | The outbound matches what the category needs. |
      | domestic_proxied | A domestic target is being proxied (pays a pointless proxy hop). |
      | foreign_direct   | A foreign target is going direct (blocked or slow). |
      | unknown          | The outbound could not be determined. |
    And answer by rules on the field "ev_path" with hit 0.9 and fallback "unknown"
      | label            | needles |
      | domestic_proxied | "domestic_proxied", "国内被代理" |
      | foreign_direct   | "foreign_direct", "国外直连" |
      | expected         | "expected", "正确", "ok" |
      | unknown          | "unknown", "不确定" |
    When the answer "expected" goes to "latency"
    And any other answer goes to "STOP"
    Then the primary question is "path_state"
    Then label "FIX_DOMESTIC_PATH" when the answer to "path_state" is "domestic_proxied"
    And label "FIX_PROXY_RULE" when the answer to "path_state" is "foreign_direct"
    And label "REVIEW" when the answer to "path_state" is "unknown"
    And label "PROBE_PATH_OK" otherwise

  Scenario: latency
    Given a choice question "latency_state" asks "What is the observed TTFB? fast < 1s, slow 1-3s, timeout > 3s or the connection failed."
      | answer  | meaning |
      | fast    | TTFB under 1 second. |
      | slow    | TTFB between 1 and 3 seconds. |
      | timeout | TTFB over 3 seconds, or no response. |
    And answer by rules on the field "ev_latency" with hit 0.9 and fallback "timeout"
      | label   | needles |
      | fast    | "fast", "快" |
      | slow    | "slow", "慢" |
      | timeout | "timeout", "超时", "失败" |
    And a choice question "category_state" asks "Is the target domestic or foreign? (used only to name the OK label)"
      | answer   | meaning |
      | domestic | A CN service / CN mirror. |
      | foreign  | A non-CN service. |
    And answer by rules on the field "ev_category" with hit 0.9 and fallback "foreign"
      | label    | needles |
      | domestic | "domestic", "国内" |
      | foreign  | "foreign", "国外", "海外" |
    When the answer "fast" goes to "STOP"
    And any other answer goes to "mirror"
    Then the primary question is "latency_state"
    Then label "OK_DIRECT" when the answer to "latency_state" is "fast" and the answer to "category_state" is "domestic"
    And label "OK_PROXY" when the answer to "latency_state" is "fast" and the answer to "category_state" is "foreign"
    And label "PROBE_LATENCY_OK" otherwise

  Scenario: mirror
    Given a choice question "mirror_state" asks "Is there a measured domestic mirror for this service (rsproxy / aliyun / tencent / tuna / hf-mirror / daocloud ...)? A slow foreign service with a fast domestic mirror should be pointed at the mirror."
      | answer    | meaning |
      | available | A domestic mirror was measured and is reachable. |
      | none      | No domestic mirror is known for this service. |
    And answer by rules on the field "ev_mirror" with hit 0.9 and fallback "none"
      | label     | needles |
      | available | "available", "有", "有镜像" |
      | none      | "none", "无", "没有" |
    When any other answer goes to "STOP"
    Then the primary question is "mirror_state"
    Then label "SUGGEST_MIRROR" when the answer to "mirror_state" is "available"
    And label "REVIEW" otherwise
