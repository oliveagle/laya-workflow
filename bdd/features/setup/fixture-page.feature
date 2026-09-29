# Shared Background steps, pulled in with `include: setup/fixture-page.feature`.
#
# This is the whole reuse mechanism in the BDD suite, and it is deliberately
# that small. Measured over all six features: 46 step lines, 37 unique, and 14
# of the 9 duplicates were these two lines in four files. So the duplication
# that actually exists is boilerplate setup, not arbitrary step text - which
# means a general $ref with parameters, conditionals and nesting would be a
# larger language to keep honest than the problem it solves.
#
# This file is a flat list of steps, not a feature: a `Feature:` header, a
# `Background:` block, or a second `include` is a parse error, so an include
# can never quietly become a recursive one.
#
# State required by the included steps: base_url
Given the browser is ready
Given I am on "<base_url>/index.html"
