Feature: rfc_editor live check
  Background:
    Given the browser is ready

  @outputs(title, url, bodytext, first_href, title2)
  Scenario: rfc_editor reads www.rfc-editor.org on the real web
    Given I am on "https://www.rfc-editor.org/info/rfc2119/"
    When I wait for the element "body"
    Then the element "body" is visible
    Then javascript "!!document.body" is true
    Then javascript "document.querySelectorAll('*').length > 10" is true
    When I extract the page title into title
    Then the saved value "title" contains text "RFC 2119"
    When I extract the page url into url
    Then the saved value "url" contains text "www.rfc-editor.org"
    Then the page url contains "rfc2119"
    When I extract the text of the element "body" into bodytext
    Then javascript "document.body.innerText.length > 40" is true
    Then javascript "document.querySelectorAll('pre, div, main').length >= 1" is true
    When I wait until the element "body" becomes visible
    Then the element "body" is visible
    When I press the key "Escape"
    When I click the element "#cookie-consent" if it is present
    Then javascript "document.title.length >= 1" is true
    Then javascript "document.querySelectorAll('a[href]').length >= 1" is true
    When I extract the attribute "href" of the element "a[href]" into first_href
    Then javascript "document.querySelector('a[href]').getAttribute('href').length >= 1" is true
    When I navigate to "https://www.rfc-editor.org/info/rfc8174/"
    When I wait for the element "body"
    Then javascript "location.href.indexOf('rfc8174') !== -1" is true
    Then the page title contains "RFC 8174"
    When I extract the page title into title2
    Then the saved value "title2" contains text "RFC 8174"
    Then the page url contains "www.rfc-editor.org"
