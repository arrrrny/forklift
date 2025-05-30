This tool allows you to create a new file or edit an existing file within the project. For moving or renaming files, use the terminal tool with the mv command instead.

**Input Fields**:
1. display_description (Required): A brief description of the edit. This field is used to summarize the purpose of the edit.
   - Example: Update copyright year in page_footer

2. path (Required): The relative path of the file to create or modify. This path must start with one of the project's root directories.
   - Example: src/main.rs

3. mode (Required): Specifies the operation mode. Possible values are:
   - edit: Make granular edits to an existing file.
   - create: Create a new file if it doesn't exist.
   - overwrite: Replace the entire contents of an existing file.

**Usage**:
- Use the read_file tool to understand the file's contents and context before editing.
- Verify the directory path is correct when creating new files:
  - Use the list_directory tool to ensure the parent directory exists and is the correct location.

**Field Requirements**:
- All fields are required for this tool to function correctly. Ensure display_description, path, and mode are provided.
