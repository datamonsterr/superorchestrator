# Antigravity accounts isolate only the .gemini directory

agy has no supported account-directory option for its login and ignores most environment variables. It also needs the real `HOME` so it can reach the system keyring. An agy account is therefore a `.gemini` directory, passed with `--gemini_dir`. On Linux, the process also runs under `bwrap`, which binds that directory over `~/.gemini`. We do not override `HOME`. This follows the approach agent-dispatcher already uses on this machine.
