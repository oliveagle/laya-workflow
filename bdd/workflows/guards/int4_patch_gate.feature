@decision
Feature: int4_patch_gate
  Port of the ole-eval DFlash2 INT4 patch-set applier
  (code/dflash2/sglang-patches/int4-support/apply_all_patches.py): the gate that
  decides whether the five INT4 DFlash2 support patches can be applied into sglang's
  site-packages -- the pre-flight step that must clear before an INT4 draft model may
  be launched. It backs up each target file, runs the five patch scripts in order, and
  exits non-zero when any one of them fails to apply, so it names its verdict rather
  than reporting a number.

  The five patches, in the source's own order, each run as its own script whose exit
  code says whether it applied:

    - save_raw_packed (00_save_raw_packed.py): save the raw GPTQ packed weight as
      weight_packed_raw *before* sglang's WNA16 Marlin repack overwrites it -- the
      format Patch 3 v2 needs for dequantization.
    - model_packed_modules (01_model_packed_modules.py): add
      `packed_modules_mapping = {"qkv_proj": ["q_proj", "k_proj", "v_proj"]}` to
      DFlashDraftModel so sglang's loader fuses a checkpoint's separate q/k/v weights.
    - dflash_utils (02_dflash_utils.py): rewrite `can_dflash_slice_qkv_weight` to accept
      quantized (INT32/INT8 packed) qkv_proj weights, not only BF16/FP16.
    - fused_kv_materialize (03_fused_kv_materialize.py): teach FusedKVMaterializeHelper
      to dequantize the INT4 qkv weight, preferring the weight_packed_raw Patch 0 saved,
      then a raw packed weight, then a BF16/FP16 weight.
    - compressed_tensors_skip (04_compressed_tensors_skip.py): skip
      process_weights_after_loading (the Marlin repack) for layers in the ignore list --
      a fallback for layers whose Marlin repack is wrong.

  The walk encodes exactly those five named checks, one node each, in source order:
  save_raw_packed -> model_packed_modules -> dflash_utils -> fused_kv_materialize ->
  compressed_tensors_skip. The first patch that fails ends the walk and names itself;
  only a run that applies all five reaches ALL_PATCHES_APPLIED.

  Three folds are documented:

    - the upstream does not stop at the first failing patch: `main` runs all five
      (`for patch in patches: if not run_patch(patch): all_ok = False`), prints every
      result, and exits 1 once, at the end, when `all_ok` is false. The graph names the
      first failure and stops; the PASS/FAIL verdict is identical either way.
    - `--reverse` is a mode, not the gate: `main` short-circuits into `reverse_patches()`
      -- restore every `*.orig` and return 0 -- and never enters the patch loop. The
      graph models the apply path the census names; the reverse path is a restore, not a
      verdict.
    - the pre-flight backup and the post-flight `clear_pycaches` are not judged: the
      backup only runs when a `.orig` is absent (and would raise, uncaught, rather than
      be an apply verdict), and `clear_pycaches` only prints a `Warning:` when `find` is
      unhappy. Neither carries the exit code; only the five patch scripts' exit codes do.

  Idempotence is only partial, and the probe really ran it: Patch 0 and Patch 3 exit 0
  when their own marker is already present (`already applied`), but a second apply is
  not clean -- Patch 1's replacement text still *contains* its own anchor, so it silently
  re-applies (duplicating `packed_modules_mapping`), while Patch 2 and Patch 4 have
  anchors that vanish, so a re-apply exits 1 with Patch 2 as the first failure. So "fail"
  means the patch script exited non-zero (its anchor block was not found), not that the
  patch was unnecessary.

  Everything the patch scripts printed stays in the harness -- the per-script stdout,
  the `.orig` backups, the found/not-found anchors. The graph reads none of it, only
  each script's exit code.

  Reference walk (apply_all_patches.py, main()): back up originals; for each of the five
  patch scripts in order, run it and record its exit code; `all_ok = all(...)`; exit 1
  when not all_ok, exit 0 otherwise.

  Background:
    Given the workflow runs at most 8 iterations

  Scenario: save_raw_packed
    Given a choice question "apply" asks "Patch 0: does 00_save_raw_packed.py apply -- does it find the `transform_w_q` block to save the raw packed weight before the Marlin repack (or see its own marker already present)? A script that exits non-zero ends the walk -- the patch set did not apply."
      | answer | meaning |
      | fail | the script exited non-zero: the transform_w_q anchor was not found |
      | pass | the script applied the patch, or its marker was already present |
    And the answer comes from the field "ev_save_raw_packed"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "model_packed_modules"
    Then the primary question is "apply"
    Then label "PATCH_SAVE_RAW_PACKED_FAILED" when the answer to "apply" is "fail"
    And label "SAVE_RAW_PACKED_OK" otherwise

  Scenario: model_packed_modules
    Given a choice question "apply" asks "Patch 1: does 01_model_packed_modules.py apply -- does it find the DFlashDraftModel class block to add `packed_modules_mapping`? A script that exits non-zero ends the walk."
      | answer | meaning |
      | fail | the script exited non-zero: the DFlashDraftModel block was not found |
      | pass | the script applied the patch |
    And the answer comes from the field "ev_model_packed_modules"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "dflash_utils"
    Then the primary question is "apply"
    Then label "PATCH_MODEL_PACKED_MODULES_FAILED" when the answer to "apply" is "fail"
    And label "MODEL_PACKED_MODULES_OK" otherwise

  Scenario: dflash_utils
    Given a choice question "apply" asks "Patch 2: does 02_dflash_utils.py apply -- does it find the original `can_dflash_slice_qkv_weight` to rewrite so it accepts quantized qkv_proj weights? A script that exits non-zero ends the walk."
      | answer | meaning |
      | fail | the script exited non-zero: the original function was not found |
      | pass | the script applied the patch |
    And the answer comes from the field "ev_dflash_utils"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "fused_kv_materialize"
    Then the primary question is "apply"
    Then label "PATCH_DFLASH_UTILS_FAILED" when the answer to "apply" is "fail"
    And label "DFLASH_UTILS_OK" otherwise

  Scenario: fused_kv_materialize
    Given a choice question "apply" asks "Patch 3: does 03_fused_kv_materialize.py apply -- does it find the v1 block to upgrade (or see its own v2 marker already present), with the dequantize helper present? A script that exits non-zero ends the walk."
      | answer | meaning |
      | fail | the script exited non-zero: neither the v2 marker nor the helper function was present |
      | pass | the script applied (or upgraded) the patch |
    And the answer comes from the field "ev_fused_kv_materialize"
    When the answer "fail" goes to "STOP"
    And any other answer goes to "compressed_tensors_skip"
    Then the primary question is "apply"
    Then label "PATCH_FUSED_KV_MATERIALIZE_FAILED" when the answer to "apply" is "fail"
    And label "FUSED_KV_MATERIALIZE_OK" otherwise

  Scenario: compressed_tensors_skip
    Given a choice question "apply" asks "Patch 4: does 04_compressed_tensors_skip.py apply -- does it find the original `process_weights_after_loading` to add the ignore-list skip? This is the last patch: a script that exits non-zero ends the walk, and applying all five means the INT4 patch set is in place."
      | answer | meaning |
      | fail | the script exited non-zero: the original method was not found |
      | pass | the script applied the patch |
    And the answer comes from the field "ev_compressed_tensors_skip"
    When any other answer goes to "STOP"
    Then the primary question is "apply"
    Then label "PATCH_COMPRESSED_TENSORS_SKIP_FAILED" when the answer to "apply" is "fail"
    And label "ALL_PATCHES_APPLIED" otherwise
