# SQLite is the system of record

All sessions, runs, events, messages, usage, and route decisions are written to one SQLite file (`state.db`, WAL mode) under the config directory. The supervisor is the only writer for runs. Wrapper launches and the CLI write their own rows directly, so usage is still recorded when the supervisor is down. The UI and the CLI read the same file. We chose an embedded database over a server database or plain JSON files because the tool must install as one binary with no services, and it must survive restarts and support timeline queries.
