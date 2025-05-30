This tool identifies errors and warnings in the codebase, either for a specific file or the entire project.

**Required Fields**:
1. Path: The relative path of the file to get diagnostics for. If omitted, the tool will return a summary of error and warning counts for all files in the project.

**Optional Fields**:
- None.

**Usage**:
1. Provide a file path to get diagnostics for that specific file.
2. Omit the file path to get a summary of error and warning counts for all files in the project.

**Examples**:
1. File Diagnostics:
   - Input: path: src/main.rs
   - Output: Detailed errors and warnings for src/main.rs.

2. Project Diagnostics:
   - Input: none
   - Output: Summary of error and warning counts across all files.

**Guidelines**:
1. Attempt to fix diagnostics up to two times. If unresolved, seek user assistance.
2. Avoid removing generated code solely to resolve diagnostics. Collaborate with the user for solutions.
