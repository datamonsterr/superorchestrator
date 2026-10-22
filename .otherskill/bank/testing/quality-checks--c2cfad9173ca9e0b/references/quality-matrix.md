# Quality matrix

## JS/TS

- Format: `bun run format`
- Lint: `bun run --filter @proompteng/bumba lint`
- Typecheck: `bun run --filter @proompteng/bumba tsc`
- Oxfmt: `bunx oxfmt --check services/bumba`

## Go

- Test: `go test ./services/prt`
- Build: `go build ./services/prt`

## Kotlin

- `./gradlew test --tests "pkg.ClassTest"`

## Rails

- `bundle exec rails test test/models/user_test.rb:42`

## Python

- `pytest alchimie_tests/test_file.py -k "pattern"`

## Targeted JS tests

- `bun run --filter @proompteng/bumba test -- services/bumba/src/activities/index.test.ts -t "bumba embeddings"`
