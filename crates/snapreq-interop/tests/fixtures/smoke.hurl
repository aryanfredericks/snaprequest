# Log in, then read the profile.
POST {{base_url}}/login
Content-Type: application/json
{
  "user": "ana",
  "password": "{{password}}"
}
HTTP 200
[Captures]
token: jsonpath "$.token" redact
session: cookie "sid"
[Asserts]
jsonpath "$.token" exists
jsonpath "$.token" isString
header "Content-Type" contains "json"
duration < 1000

GET {{base_url}}/me
Authorization: Bearer {{token}}
[Options]
retry: 2
retry-interval: 100ms
HTTP 200
[Asserts]
jsonpath "$.name" == "ana"
jsonpath "$.roles" count == 2
jsonpath "$.id" > 0
jsonpath "$.email" matches /^[a-z]+@/
xpath "//title" exists  # not supported

DELETE {{base_url}}/session
[BasicAuth]
ana: s3cret-basic
HTTP 204
