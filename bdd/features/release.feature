# Executed by: scripts/bdd/run.py
# State required: base_url
#
# `release` is the sixth op, and it was the only one no scenario ran - it
# shipped in the plugin and was listed in the op docs, but a .feature could
# not say it, so nothing in the suite ever closed a page. The same hole
# `I run javascript` was in, one level down.
#
# The green path is the interesting one. `bdd.release` throws unless the engine
# actually closed a tab, so "this scenario passed" *is* the claim that a real
# page went away. Before that fix the op returned `released: true`
# unconditionally and could not fail, which is exactly why it was never worth
# running: there was nothing in it to be wrong about.
#
# The dishonest case - releasing a target that is already closed - is not
# expressible here on purpose. After a release the compiler knows there is no
# page, so a second release is a compile error rather than a run. That failure
# is reached from a hand-written spec in dsl/browser/bdd_release_probe.json,
# because the point of that probe is to reach the states the vocabulary forbids.
Feature: Releasing a page
  Background:
    Given the browser is ready
    Given I am on "<base_url>/index.html"

  Scenario: Releasing closes the tab the scenario was using
    Then the element "#heading" is visible
    When I release the page
    # Nothing may follow this line. `I release the page` sets the compiler's
    # has-no-page state, so any later step is "needs a page, but no earlier
    # step opened one" at compile time. There is no green way to say more.
