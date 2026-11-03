# Running SnapRequest in CI

`snapreq run` is made for pipelines: it exits non-zero when anything fails and writes JUnit XML that
CI systems display, plus an HTML report to keep as an artifact.

## Exit codes

| Code | `snapreq send` | `snapreq run` |
|---|---|---|
| 0 | A response was received (any status, unless `--fail`). | Every request, step and check passed. |
| 1 | Internal error. | Internal error, or a report could not be written. |
| 2 | Invalid command line. | Invalid command line (for example `-n` with `--data`). |
| 3 | Invalid request; nothing was sent. | A request could not be built (undefined variable, bad file, unknown target). |
| 4 | `--fail` and a status of 400 or higher. | — |
| 5 | Network failure (DNS, connect, TLS, protocol). | A request failed on the network, or no OAuth token could be obtained. |
| 6 | Timeout. | A request timed out (and no worse problem happened). |
| 7 | An `[assert]` check of the request file failed. | A check or capture failed. |

For `snapreq run` the worst problem decides: 3, then 5, then 6, then 7.

## Secrets

Declare secret variables in the collection (`password = { secret = true }`) and give their values to
the job as `SNAPREQ_SECRET_<NAME>` environment variables (upper case, `.` and `-` become `_`) from the
CI system's secret store. They are hidden in all output and reports.

## GitHub Actions

```yaml
name: API tests
on: [push, pull_request]

jobs:
  api:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      checks: write          # for the JUnit check run
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      # Until release binaries exist, build snapreq from source:
      - run: cargo install --locked --git https://github.com/aryanfredericks/snaprequest snapreq-cli
      - name: Run the collection and the checkout flow
        env:
          SNAPREQ_SECRET_PASSWORD: ${{ secrets.API_PASSWORD }}
        run: |
          status=0
          snapreq run api -e staging --report junit:reports/collection.xml --report html:reports/collection.html || status=$?
          snapreq run api/orders/checkout.flow.toml -e staging --report junit:reports/checkout.xml --report html:reports/checkout.html || status=$?
          exit $status
      - uses: mikepenz/action-junit-report@v5
        if: always()
        with:
          report_paths: reports/*.xml
      - uses: actions/upload-artifact@v4
        if: always()
        with:
          name: api-reports
          path: reports/
```

Running both commands before failing the step (`|| status=$?`) keeps the reports of the second run
even when the first one fails.

## In this repository

`examples/shop/` is a small collection with a checkout flow, run by the `api-tests` CI job against an
in-memory mock (`crates/snapreq-test/examples/mock_shop.rs`). Run it locally with
`scripts/api-tests.sh`; reports go to `target/api-reports/`.
