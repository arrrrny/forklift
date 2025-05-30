Moves or renames a file or directory in the project and confirms success. If the source and destination directories are the same but the filename is different, this performs a rename. Otherwise, it performs a move.

**Input Fields**:
1. **Source Path** (Required): The relative path of the file or directory to move or rename. This must start with one of the project's root directories.
   - Example: To move a file located in directory1/a/something.txt, specify directory1/a/something.txt as the source path.
2. **Destination Path** (Required): The relative path where the file or directory should be moved or renamed to. If the paths are the same except for the filename, this will perform a rename.
   - Example: To rename directory1/a/something.txt to directory1/a/renamed.txt, specify directory1/a/renamed.txt as the destination path.

**Usage**:
Use this tool to relocate or rename files or directories efficiently without altering their contents. Ensure the paths are valid and within the project boundaries.
