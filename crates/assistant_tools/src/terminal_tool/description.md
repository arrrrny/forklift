Executes a shell one-liner and returns the combined output.

**Input Fields**:
1. Command (Required): The shell command to execute. This must be a single-line command that terminates on its own. Avoid commands that run indefinitely, such as servers or file watchers.
2. CD (Required): The working directory for the command. Specify one of the project's root directories. Do not include directory navigation in the command itself.

**Behavior**:
- Spawns a process using the user's shell and captures both stdout and stderr, preserving the order of writes.
- Returns a string with the combined output result.
- Each invocation is isolated; no state is preserved between calls.

**Usage Guidelines**:
- Avoid redundancy by not listing output already shown to the user.
- Ensure the CD parameter is used for directory navigation; commands with embedded navigation will fail.
- Do not use this tool for commands that run indefinitely, such as npm run start or python -m http.server.
