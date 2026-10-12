@decision
Feature: cua_desktop

  Background:
    Given the workflow runs at most 48 iterations

  Scenario: decide
    Given a choice question "go" asks "Run the Cua Driver call the plugin planned? Answer B to stop here. Routing itself is decided by the `route` node from `step_verdict`."
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When the answer "A" goes to "route"
    And any other answer goes to "STOP"
    Then call the capability "cua"
      | with.op      | plan |
      | with.intent      | ${state.intent} |
      | with.session      | ${state.session} |
      | with.call_index      | ${state.turn} |
      | with.max_calls      | ${state.max_calls} |
      | with.allow_danger      | ${state.allow_danger} |
      | with.pid      | ${state.pid} |
      | with.window_id      | ${state.window_id} |
      | with.observation      | ${state.raw_result} |
      | with.error_text      | ${state.raw_error} |
      | with.exit_code      | ${state.driver_exit} |
      | with.keys_pressed      | ${state.keys_pressed} |
      | with.motion_configured      | ${state.motion_configured} |
      | with.click_mode      | ${state.click_mode} |
      | with.click_debug_image_out      | ${state.click_debug_image_out} |
      | with.observe_screenshot_out_file      | ${state.observe_screenshot_out_file} |
      | project.status  | /status |
      | project.step_tool  | /tool |
      | project.step_args  | /args_json |
      | project.step_risk  | /risk |
      | project.step_phase  | /phase |
      | project.step_why  | /why |
      | project.step_error  | /error |
      | project.step_verb  | /verb |
      | project.step_blocked  | /blocked |
      | project.step_candidates  | /candidates |
      | project.step_alternatives  | /alternatives |
      | project.step_matched  | /matched_text |
      | project.turn  | /turn |
      | project.done  | /done |
      | project.cua_log  | /cua_log |
      | project.driver_status  | /driver_status |
      | project.step_verdict  | /verdict |
      | project.pid  | /pid |
      | project.window_id  | /window_id |
      | project.keys_pressed  | /keys_pressed |
      | project.motion_configured  | /motion_configured |

  Scenario: route
    Given a choice question "go" asks "The planner returned step_verdict=GO. Run it, or stop?"
      | answer | meaning              |
      | A      | run the planned Cua call       |
      | B      | stop                           |
    And answer "B" when the field "step_verdict" contains "HALT" with hit 0.9 and miss 0.05
    When the answer "A" goes to "exec"
    And the answer "B" goes to "STOP"
    And any other answer goes to "STOP"

  Scenario: exec
    Given a choice question "seen" asks "Continue the loop with the result of this Cua Driver call?"
      | answer | meaning              |
      | A      | yes                            |
      | B      | no                             |
    When the answer "A" goes to "decide"
    And any other answer goes to "STOP"
    Then call the capability "driver"
      | with.op      | call |
      | project.raw_result  | /stdout |
      | project.driver_exit  | /exit_code |
      | project.argv_used  | /argv |
      | project.raw_error  | /stderr |

