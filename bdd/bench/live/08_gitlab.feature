Feature: gitlab live check
  Background:
    Given the browser is ready

  @outputs(title, url, bodytext, first_href, title2)
  Scenario: gitlab reads gitlab.com on the real web
    Given I am on "https://gitlab.com/gitlab-org/gitlab"
    When I wait for the element "body"
    Then the element "body" is visible
    Then javascript "!!document.body" is true
    Then javascript "document.querySelectorAll('*').length > 10" is true
    When I extract the page title into title
    Then the saved value "title" contains text "GitLab"
    When I extract the page url into url
    Then the saved value "url" contains text "gitlab.com"
    Then the page url contains "gitlab-org/gitlab"
    When I extract the text of the element "body" into bodytext
    Then javascript "document.body.innerText.length > 40" is true
    Then javascript "document.querySelectorAll('div, a').length > 10" is true
    When I wait until the element "body" becomes visible
    Then the element "body" is visible
    When I press the key "Escape"
    When I click the element "#cookie-consent" if it is present
    Then javascript "document.title.length >= 1" is true
    Then javascript "document.querySelectorAll('a[href]').length >= 1" is true
    When I extract the attribute "href" of the element "a[href]" into first_href
    Then javascript "document.querySelector('a[href]').getAttribute('href').length >= 1" is true
    When I navigate to "https://gitlab.com/gitlab-org/gitlab-runner"
    When I wait for the element "body"
    Then javascript "location.href.indexOf('gitlab-runner') !== -1" is true
    Then the page title contains "gitlab-runner"
    When I extract the page title into title2
    Then the saved value "title2" contains text "gitlab-runner"
    Then the page url contains "gitlab.com"
