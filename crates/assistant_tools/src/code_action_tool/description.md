A tool for applying code actions to specific sections of your code. It integrates with language servers to provide advanced refactoring capabilities similar to those found in modern IDEs.

**Input Fields**

1. Path (Required): The relative path of the file where the code action should be applied. This must start with one of the project's root directories. Example: src/main.rs.
2. Action (Optional): The specific code action to apply. If left empty, the tool will list all available actions for the specified range. Examples include quickfix.all or source.organizeImports.
3. Arguments (Optional): Additional arguments required by the action. For example, when renaming, provide the new name as an argument. Example: newName updatedVariable.
4. Context Before Range (Required): The text immediately preceding the range where the action is applied. This helps identify the correct location. Example: let x equals.
5. Text Range (Required): The exact range of text where the action is applied, specified as line and character positions. Example: start line 10 character 5, end line 10 character 15.
6. Context After Range (Required): The text immediately following the range where the action is applied. This ensures the tool targets the correct code section. Example: semicolon.

**Usage**

Use this tool to explore available code actions for a specific piece of code. Apply automatic fixes, refactorings, or transformations to improve code quality. Rename variables, functions, or other symbols consistently across your project. Clean up imports, implement interfaces, or perform other language-specific operations.

**Examples**

To rename a variable, specify the path, action, arguments, and context. To organize imports, specify the path and action. To apply all quick fixes, specify the path and action.

The tool automatically saves any changes it makes to your files.
