Searches the contents of files in the project using a regular expression.

**Input Fields**:
1. Regex (Required): The pattern to search for within file contents. Supports full regex syntax such as log.*Error or function\\\\s+\\\\w+.
2. Include Pattern (Optional): Glob pattern to narrow the search to specific files or directories, for example **/*.rs for Rust files.
3. Offset (Optional): Starting position for paginated results, starting at zero by default.
4. Case Sensitive (Optional): Flag to enable case-sensitive matching, false by default.

**Usage**:
Use this tool to locate specific patterns in file contents, such as function definitions or error logs. Prefer this tool over path search when searching for symbols, as it directly scans file contents. Results are paginated with 20 matches per page. Use the offset parameter to navigate pages.

**Examples**:
To find all occurrences of Error in Rust files, search using Regex Error and Include Pattern for Rust files. To perform a case-sensitive search for TODO, enable Case Sensitive and search using Regex TODO.
