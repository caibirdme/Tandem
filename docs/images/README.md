# README screenshots

These images show the actual Tandem React interface in English, captured in browser development mode with an isolated `tandem-dev-server` data directory. Conversation messages, project names, model settings, and SSH details are illustrative. No real provider credentials, private conversations, or remote hosts are used.

| Image                   | View                                                                                                 |
| ----------------------- | ---------------------------------------------------------------------------------------------------- |
| `work-session.jpg`      | A local project conversation with model selection and the default **Ask each time** permission mode. |
| `provider-settings.jpg` | The provider editor's request URL preview, API key field, and model configuration.                   |
| `ssh-project.jpg`       | The remote project form with example SSH connection details and a working directory.                 |

To refresh the images:

1. Start the frontend and development service as described in [Development](../development.md), using a separate temporary `--data-dir` for the service.
2. Use fictional projects and sample conversation content. Keep API keys empty and use reserved example domains such as `api.example.com` and `dev.example.com`.
3. Capture the complete application viewport with the sidebar visible for the work-session image. Frame setup screenshots around their dialogs, scrolling to keep the relevant fields visible. Leave example connection details unsaved and do not contact example hosts.
4. Save browser captures as JPEG files with the names above. Check every image at the width used in the [README](../../README.md), and keep its alt text and caption consistent with the visible interface.

The screenshots demonstrate the interface; they do not establish inference, tool execution, or SSH integration results. See [Verification](../verification.md) for tested behavior.
