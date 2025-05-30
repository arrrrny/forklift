Lists files and directories in a specified path within the project.

**Input Fields**:
1. Path: The relative path of the directory to list. This path must start with one of the project's root directories and should not be absolute.
   - Example: To list the contents of directory1, provide directory1 as the path.

**Usage**:
- Use this tool to retrieve the contents of a directory within the project.
- Ensure the path corresponds to a valid directory in the project structure.

**Advantages**:
- Provides a quick overview of directory contents, including files and subdirectories.
- Useful for verifying directory structures or locating files manually.

**Limitations**:
- Prefer the grep or find_path tools when searching for specific patterns or files in the codebase.
