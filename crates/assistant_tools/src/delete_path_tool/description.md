This tool deletes a file or directory, including all its contents, at the specified path within the project.

**Input Fields**:
1. Path (Required): The relative path of the file or directory to delete. This must start with one of the project's root directories.
   - Example: To delete a file in directory1, provide the path directory1/a/something.txt.

**Usage**:
- Use this tool to remove unwanted files or directories.
- Ensure the path is valid and exists within the project boundaries.
- This tool requires the "Path" field to be specified for successful execution.

**Behavior**:
- Returns a confirmation message upon successful deletion.
- If the path is invalid or outside the project, the tool will fail.
- Deletes directories recursively, ensuring all contents are removed.
- Operates exclusively on paths within the project boundaries.
