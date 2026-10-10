Feature: php_net live check
  Background:
    Given the browser is ready

  @outputs(title, url, bodytext, first_href, title2)
  Scenario: php_net reads www.php.net on the real web
    Given I am on "https://www.php.net/"
    When I wait for the element "body"
    Then the element "body" is visible
    Then javascript "!!document.body" is true
    Then javascript "document.querySelectorAll('*').length > 10" is true
    When I extract the page title into title
    Then the saved value "title" contains text "PHP"
    When I extract the page url into url
    Then the saved value "url" contains text "www.php.net"
    Then the page url contains "php.net"
    When I extract the text of the element "body" into bodytext
    Then javascript "document.body.innerText.length > 40" is true
    Then javascript "document.querySelectorAll('nav, main, div').length >= 1" is true
    When I wait until the element "body" becomes visible
    Then the element "body" is visible
    When I press the key "Escape"
    When I click the element "#cookie-consent" if it is present
    Then javascript "document.title.length >= 1" is true
    Then javascript "document.querySelectorAll('a[href]').length >= 1" is true
    When I extract the attribute "href" of the element "a[href]" into first_href
    Then javascript "document.querySelector('a[href]').getAttribute('href').length >= 1" is true
    When I navigate to "https://www.php.net/downloads"
    When I wait for the element "body"
    Then javascript "location.href.indexOf('downloads') !== -1" is true
    Then the page title contains "Downloads"
    When I extract the page title into title2
    Then the saved value "title2" contains text "Downloads"
    Then the page url contains "www.php.net"
