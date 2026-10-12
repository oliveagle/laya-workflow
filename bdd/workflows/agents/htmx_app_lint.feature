@decision
Feature: htmx_app_lint

  Background:
    Given the workflow runs at most 2 iterations

  Scenario: decision
    Given a choice question "yaml_schema_ok" asks "Did app.yaml pass the schema check (apiVersion=webui/v1, app.name+title, routes[].path+template, auth.mode=oauth)?"
      | answer | meaning              |
      | A      | schema OK                      |
      | B      | schema violation(s)            |
    And answer "B" when the text contains "yaml_schema_fail", "app_yaml_missing", "routes_empty", "auth_mode_non_oauth", "api_version_wrong" with hit 0.95
    And a choice question "templates_present" asks "Does templates/ exist and contain at least one .html file?"
      | answer | meaning              |
      | A      | templates present              |
      | B      | templates missing or empty     |
    And answer "B" when the text contains "templates_missing", "no_templates_under" with hit 0.95
    And a choice question "templates_parse_ok" asks "Did all templates/*.html parse via Go text/template (chrome + chrome-partial defines)?"
      | answer | meaning              |
      | A      | all parse OK                   |
      | B      | parse error(s)                 |
    And answer "B" when the text contains "template_parse_err", "template_read_err", "no_templates_under" with hit 0.95
    And a choice question "back_button_advisory" asks "Is ui.header.back_button.enabled explicitly false (no-effect advisory per ADR §D6#4)?"
      | answer | meaning              |
      | A      | no advisory                    |
      | B      | back_button=false advisory     |
    And answer "B" when the text contains "back_button_advisory", "back_button_disabled" with hit 0.85
    When any other answer goes to "STOP"
    Then label "FAIL_YAML_SCHEMA" when the answer to "yaml_schema_ok" is "B"
    And label "FAIL_TEMPLATES_MISSING" when the answer to "templates_present" is "B"
    And label "FAIL_TEMPLATE_PARSE" when the answer to "templates_parse_ok" is "B"
    And label "WARN" when the answer to "back_button_advisory" is "B"
    And label "PASS" otherwise
    And keep the state key "yaml_schema_fail"
    And keep the state key "templates_missing"
    And keep the state key "template_parse_err"
    And keep the state key "back_button_advisory"
    And keep the state key "app_dir"

