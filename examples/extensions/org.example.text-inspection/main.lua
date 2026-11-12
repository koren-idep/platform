local extension = {}

function extension.activate()
  koren.inspections.register_rule({
    id = "todo-marker",
    title = "TODO marker",
    description = "Find TODO markers in plain text documents.",
    category = "maintainability",
    languages = {"plaintext"},
    severity = "warning"
  }, function(document, settings)
    local findings = {}
    if not settings.enabled then
      return findings
    end

    local start_at, end_at = document.text:find("TODO", 1, true)
    while start_at do
      findings[#findings + 1] = {
        start_line = 0,
        start_column = start_at - 1,
        end_line = 0,
        end_column = end_at,
        message = "TODO marker remains",
        category = "maintainability",
        explanation = "Replace or remove this marker before sharing the document.",
        replacement = "DONE"
      }
      start_at, end_at = document.text:find("TODO", end_at + 1, true)
    end
    return findings
  end)
end

return extension
