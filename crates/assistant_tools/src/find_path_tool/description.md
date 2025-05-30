Fast file path pattern matching tool that works with any codebase size

**Features**
- Supports glob patterns like **/*.js or src/**/*.ts
- Returns matching file paths sorted alphabetically
- Prefer the grep tool to this tool when searching for symbols unless you have specific information about paths.
- Use this tool when you need to find files by name patterns.
- Results are paginated with 50 matches per page. Use the optional offset parameter to request subsequent pages.

**Input Fields**
1. **glob** (Required): The glob pattern to match against every path in the project. Example: **/*.rs for Rust files or src/**/*.ts for TypeScript files.
2. **offset** (Optional): The starting position for paginated results, beginning at 0 by default.

**Usage**
Use this tool to locate files efficiently based on their names or extensions. It is ideal for organizing large codebases or finding specific files without opening directories manually.
