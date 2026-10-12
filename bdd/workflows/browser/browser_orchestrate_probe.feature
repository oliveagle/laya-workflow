@decision
Feature: browser_orchestrate_probe
  Port of browser_orchestrate_probe: converted from dsl/browser/browser_orchestrate_probe.json (BDD round-trip).

  Background:
    Given the workflow runs at most 8 iterations
    Given the policy
      | allow_exec | true |
      | allow_hosts | "127.0.0.1", "localhost" |
      | max_timeout_ms | 180000 |
      | max_output | 262144 |
      | retries | 0 |
    Given the capabilities
      | chrome_ensure | {"kind":"exec","argv":["laya-workflow","browser","ensure","--backend","chrome","--port","${state.cdp_port}"],"timeout_ms":30000,"max_output":4096} |
      | server_ensure | {"kind":"exec","argv":["laya-workflow","server","ensure","--port","${state.port}","--health-path","/","--command","${state.server_cmd}"],"timeout_ms":120000,"max_output":8192} |
      | chrome | {"kind":"chrome_cdp","endpoint":"http://127.0.0.1:${state.cdp_port}","launch":true,"startup_timeout_ms":20000,"timeout_ms":15000,"max_text":6000,"max_owned_pages":32,"owned_idle_ms":300000} |
      | page_open | {"kind":"plugin","plugin":"browser_base","op":"open","browser":"chrome","timeout_ms":60000} |
      | page_htmx | {"kind":"plugin","plugin":"browser_base","op":"wait_htmx","browser":"chrome","timeout_ms":60000} |
      | page_assert | {"kind":"plugin","plugin":"browser_base","op":"assert","browser":"chrome","timeout_ms":60000} |

  Scenario: ensure_chrome
    Given a choice question "ok" asks "Run `laya-workflow browser ensure --backend chrome --port <cdp_port>` (idempotent: does nothing when a CDP endpoint already listens, otherwise launches a dedicated Chrome)?"
      | answer | meaning |
      | A | yes, the subcommand exited 0 and the CDP endpoint is (still) up |
      | B | no, the script failed or the CDP endpoint is unreachable |
    When any other answer goes to "ensure_server"
    Then call the capability "chrome_ensure"
      | project.chrome_ok | /ok |
      | project.chrome_exit | /exit_code |
      | project.chrome_msg | /stdout |
    Then keep the state key "repo"
    Then keep the state key "cdp_port"
    Then keep the state key "port"
    Then keep the state key "server_cmd"
    Then keep the state key "url"

  Scenario: ensure_server
    Given a choice question "ok" asks "Run `laya-workflow server ensure` (idempotent: returns immediately when a server already answers on the port, otherwise cold-starts state.server_cmd as a daemon and waits for it)?"
      | answer | meaning |
      | A | yes, a server is now guaranteed on the port |
      | B | no, the script failed to guarantee a server |
    When any other answer goes to "open"
    Then call the capability "server_ensure"
      | project.server_ok | /ok |
      | project.server_exit | /exit_code |
      | project.server_base | /stdout |
    Then keep the state key "repo"
    Then keep the state key "cdp_port"
    Then keep the state key "port"
    Then keep the state key "server_cmd"
    Then keep the state key "url"

  Scenario: open
    Given a choice question "ok" asks "Open the ensured URL in a real Chrome page and wait until it is actually loaded (URL matches + readyState complete)?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "wait_htmx"
    Then call the capability "page_open"
      | with.url | ${state.url} |
      | project.page_url | /url |
      | project.page_title | /title |
      | project.page_sample | /sample |
    Then keep the state key "repo"
    Then keep the state key "cdp_port"
    Then keep the state key "port"
    Then keep the state key "server_cmd"
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
    Then keep the state key "repo"
    Then keep the state key "cdp_port"
    Then keep the state key "port"
    Then keep the state key "server_cmd"
    Then keep the state key "url"

  Scenario: assert
    Given a choice question "ok" asks "Run the generic rendered-page assertion (non-empty title and body text) and require it to pass?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "done"
    Then call the capability "page_assert"
      | with.url | ${state.url} |
      | with.expression | "({ title_ok: (document.title \u007c\u007c \"\").length > 0, text_ok: !!(document.body && document.body.innerText.length > 0), url: location.href, sample: document.body ? document.body.innerText.slice(0, 200) : \"\" })" |
      | with.checks | {"title_ok":true,"text_ok":true} |
      | project.assert_url | /url |
      | project.assert_sample | /sample |
      | project.assert_pass | /pass |
    Then keep the state key "repo"
    Then keep the state key "cdp_port"
    Then keep the state key "port"
    Then keep the state key "server_cmd"
    Then keep the state key "url"

  Scenario: done
    Given a choice question "done" asks "Is the browser orchestration probe complete after both ensure steps ran and the page loaded, htmx was reported, and the render assertion passed?"
      | answer | meaning |
      | A | yes |
      | B | no |
    When any other answer goes to "STOP"
    Then keep the state key "repo"
    Then keep the state key "cdp_port"
    Then keep the state key "port"
    Then keep the state key "server_cmd"
    Then keep the state key "url"
