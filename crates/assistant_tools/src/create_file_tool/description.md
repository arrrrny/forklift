Creates a new file at the specified path within the project and writes the provided text content into it. Confirms success upon creation.

**How It Works**:
- Specify the path where the file should be created. This must start with one of the project's root directories.
- Provide the text content to be written into the new file.

**Input Fields**:
1. Path (Required): The relative path where the file should be created. Example: To create a file in directory1, specify directory1/new_file.txt. Ensure the path is valid and within the project boundaries.

2. Contents (Required): The text content to be written into the new file. Example: To create a file with Hello World, specify Hello World as the contents.

**Usage**:
Use this tool to create new files with specific text content or replace the entire contents of an existing file. It is ideal for initializing files or overwriting existing ones completely.

**Limitations**:
Do not use this tool for partial edits to existing files. Use an editing tool for such cases.