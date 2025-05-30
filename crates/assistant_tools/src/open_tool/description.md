This tool opens a file or URL using the default application on the user's operating system. It supports various platforms:
- macOS: Uses the open command.
- Windows: Uses the start command.
- Linux: Uses commands like xdg-open, gio open, gnome-open, kde-open, or wslview.

**Input Fields**:
1. path_or_url (Required): Represents the file path or URL to open. For example:
   - To open a web browser with a URL, provide https://example.com.
   - To open a PDF file, provide the file path, such as documents/report.pdf.

**Usage Guidelines**:
- Use this tool only when explicitly requested by the user.
- Ensure the path_or_url field is valid and accessible within the user's environment.
- Do not assume the user wants something opened without their instruction.

This tool is ideal for opening files or URLs in their associated applications, ensuring seamless integration with the user's environment.
