@decision
Feature: mlu270_smoke

  Background:
    Given the workflow runs at most 2 iterations

  Scenario: decision
    Given a choice question "cntool_ok" asks "Did cntool enumerate MLU270 devices?"
      | answer | meaning              |
      | A      | cntool OK                      |
      | B      | cntool missing/failed          |
    And answer "B" when the text contains "cntool_missing", "cntool_failed", "mlu270_not_detected" with hit 0.95
    And a choice question "cntoolkit_sample_ok" asks "Did cntoolkit sample inference (matrix multiply on MLU) succeed?"
      | answer | meaning              |
      | A      | sample ran                     |
      | B      | sample failed                  |
    And answer "B" when the text contains "cntoolkit_sample_failed", "sample_compile_failed" with hit 0.95
    And a choice question "torch_mlu_ok" asks "Did torch_mlu import + device probe succeed?"
      | answer | meaning              |
      | A      | import OK                      |
      | B      | import/device failed           |
    And answer "B" when the text contains "torch_mlu_import_failed", "torch_mlu_device_failed" with hit 0.95
    And a choice question "inference_smoke_ok" asks "Did the matmul+layernorm+softmax smoke run on mlu:0 complete?"
      | answer | meaning              |
      | A      | smoke passed                   |
      | B      | smoke failed                   |
    And answer "B" when the text contains "inference_smoke_failed", "no_available_runtime" with hit 0.95
    And a choice question "driver_warn" asks "Did driver / firmware / torch versions trigger an advisory warn?"
      | answer | meaning              |
      | A      | no advisory                    |
      | B      | version advisory               |
    And answer "B" when the text contains "version_mismatch_warn", "driver_advisory" with hit 0.85
    When any other answer goes to "STOP"
    Then label "FAIL" when the answer to "cntool_ok" is "B"
    And label "FAIL" when the answer to "cntoolkit_sample_ok" is "B"
    And label "FAIL" when the answer to "torch_mlu_ok" is "B"
    And label "FAIL" when the answer to "inference_smoke_ok" is "B"
    And label "WARN" when the answer to "driver_warn" is "B"
    And label "PASS" otherwise
    And keep the state key "cntool_missing"
    And keep the state key "cntoolkit_sample_failed"
    And keep the state key "torch_mlu_import_failed"
    And keep the state key "inference_smoke_failed"
    And keep the state key "no_available_runtime"
    And keep the state key "version_mismatch_warn"

