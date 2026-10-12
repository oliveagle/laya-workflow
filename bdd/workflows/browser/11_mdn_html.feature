Feature: mdn_html live check
  Background:
    Given the browser is ready

  @outputs(title, url, bodytext, first_href, title2)
  Scenario: mdn_html reads developer.mozilla.org on the real web
    Given I am on "https://developer.mozilla.org/en-US/docs/Web/HTML"
    When I wait for the element "body"
    Then the element "body" is visible
    Then javascript "!!document.body" is true
    Then javascript "document.querySelectorAll('*').length > 10" is true
    When I extract the page title into title
    Then the saved value "title" contains text "HTML"
    When I extract the page url into url
    Then the saved value "url" contains text "developer.mozilla.org"
    Then the page url contains "Web/HTML"
    When I extract the text of the element "body" into bodytext
    Then javascript "document.body.innerText.length > 40" is true
    Then javascript "document.querySelectorAll('article, main, div').length >= 1" is true
    When I wait until the element "body" becomes visible
    Then the element "body" is visible
    When I press the key "Escape"
    When I click the element "#cookie-consent" if it is present
    Then javascript "document.title.length >= 1" is true
    Then javascript "document.querySelectorAll('a[href]').length >= 1" is true
    When I extract the attribute "href" of the element "a[href]" into first_href
    Then javascript "document.querySelector('a[href]').getAttribute('href').length >= 1" is true
    When I navigate to "https://developer.mozilla.org/en-US/docs/Web/CSS"
    When I wait for the element "body"
    Then javascript "location.href.indexOf('Web/CSS') !== -1" is true
    Then the page title contains "CSS"
    When I extract the page title into title2
    Then the saved value "title2" contains text "CSS"
    Then the page url contains "developer.mozilla.org"
