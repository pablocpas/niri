# tiri-visual-tests

> [!NOTE]
>
> This is a development-only app, you shouldn't package it.

This app contains a number of hard-coded test scenarios for visual inspection.
It uses the real tiri layout and rendering code, but with mock windows instead of Wayland clients.
The idea is to go through the test scenarios and check that everything *looks* right.

## Running

You will need recent GTK and libadwaita.
Then, `cargo run`.

For CI or local smoke runs without manual interaction:

```bash
xvfb-run -a cargo run -p tiri-visual-tests -- --smoke-test
```

You can tune time spent in each case:

```bash
xvfb-run -a cargo run -p tiri-visual-tests -- --smoke-test --smoke-test-case-ms 150
```
