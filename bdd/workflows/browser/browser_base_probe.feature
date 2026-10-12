@decision
Feature: browser_base_probe
  Port of browser_base_probe: converted from dsl/browser/browser_base_probe.json (BDD round-trip).

  Background:
    Given the workflow runs at most 5 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts | "127.0.0.1", "localhost" |
      | max_timeout_ms | 120000 |
      | max_output | 262144 |
      | retries | 0 |
    Given the capabilities
      | chrome | {"kind":"chrome_cdp","endpoint":"http://127.0.0.1:${state.cdp_port}","launch":true,"startup_timeout_ms":20000,"timeout_ms":15000,"max_text":6000,"max_owned_pages":32,"owned_idle_ms":300000,"profile_dir":"${state.cdp_profile}","chrome_binary":"scripts/bdd/chrome-headless.sh"} |
      | page_open | {"kind":"plugin","plugin":"browser_base","op":"open","browser":"chrome","timeout_ms":60000} |
      | page_htmx | {"kind":"plugin","plugin":"browser_base","op":"wait_htmx","browser":"chrome","timeout_ms":60000} |
      | page_assert | {"kind":"plugin","plugin":"browser_base","op":"assert","browser":"chrome","timeout_ms":60000} |

  Scenario: open
    Given a choice question "ok" asks "Open the target URL in a real Chrome page and wait until it is actually loaded (URL matches + readyState complete)?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "wait_htmx"
    Then call the capability "page_open"
      | with.url | ${state.url} |
      | project.page_url | /url |
      | project.page_title | /title |
      | project.page_sample | /sample |
    Then keep the state key "url"

  Scenario: wait_htmx
    Given a choice question "ok" asks "Wait up to 15s for window.htmx to become defined on the page, then report whether it loaded?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "assert"
    Then call the capability "page_htmx"
      | with.url | ${state.url} |
      | project.htmx_loaded | /htmx_loaded |
      | project.htmx_url | /url |
      | project.htmx_title | /title |
    Then keep the state key "url"

  Scenario: assert
    Given a choice question "ok" asks "Run the generic rendered-page assertion (non-empty title and body text) and require it to pass?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "done"
    Then call the capability "page_assert"
      | with.url | ${state.url} |
      | with.expression | "({ title_ok: (document.title \u007c\u007c \"\").length > 0, text_ok: !!(document.body && document.body.innerText.length > 0), htmx_ok: typeof window.htmx === \"object\", url: location.href, sample: document.body ? document.body.innerText.slice(0,200) : \"\" })" |
      | with.checks | {"title_ok":true,"text_ok":true,"htmx_ok":true} |
      | project.assert_url | /url |
      | project.assert_sample | /sample |
      | project.assert_pass | /pass |
    Then keep the state key "url"

  Scenario: done
    Given a choice question "done" asks "Is the browser_base probe complete after the page loaded, htmx was reported, and the render assertion passed?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then keep the state key "url"
