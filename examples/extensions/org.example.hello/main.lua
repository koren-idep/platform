local extension = {}

function extension.activate()
  koren.commands.register(
    "hello",
    "Hello from Koren",
    "Show the current workspace path",
    function(_arguments, context)
      return "Hello from " .. (context.workspace_root or "an empty workspace")
    end
  )
  koren.ui.add_menu_action("hello", "Hello from Koren", "tools")
end

function extension.deactivate()
  -- The host removes command and menu contributions after this callback.
end

return extension

