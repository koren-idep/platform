local extension = {}

function extension.activate()
  koren.commands.register_with_options(
    "hello",
    "Hello from Koren",
    "Show the current workspace path",
    { shortcut = "Ctrl+Alt+H", enablement = "always" },
    function(_arguments, context)
      local root = context.workspace_root or "an empty workspace"
      local display = koren.settings.get("show_path") and ("Workspace: " .. root) or "Workspace path hidden by settings."
      koren.ui.update_tool_window("workspace", display)
      return "Hello from " .. root
    end
  )
  koren.ui.add_menu_action("hello", "Hello from Koren", "tools")
  koren.ui.add_menu_action("hello", "Hello from Koren", "command_palette")
  koren.ui.register_tool_window("workspace", "Workspace Info", "Run Hello from Koren to refresh workspace details.")
  koren.settings.register_boolean("show_path", "Show workspace path", true)
  koren.themes.register("midnight-blue", "Midnight Blue", "dark", {
    background = "#080b12",
    panel = "#101624",
    accent = "#62a8ff",
    text = "#e5edf8"
  })
  koren.diagnostics.provide_detail("general-help", nil, nil, "Developer Tools", "This example adds optional diagnostic context without changing the source diagnostic.")
end

function extension.deactivate()
  -- The host removes command and menu contributions after this callback.
end

return extension

