local extension = {}

function extension.activate()
  local function utf16_units(value)
    local units = 0
    for _, codepoint in utf8.codes(value) do
      units = units + (codepoint > 0xFFFF and 2 or 1)
    end
    return units
  end

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

    local line_number = 0
    for line in (document.text .. "\n"):gmatch("(.-)\n") do
      local start_at, end_at = line:find("TODO", 1, true)
      while start_at do
        findings[#findings + 1] = {
          start_line = line_number,
          start_column = utf16_units(line:sub(1, start_at - 1)),
          end_line = line_number,
          end_column = utf16_units(line:sub(1, end_at)),
          message = "TODO marker remains",
          category = "maintainability",
          explanation = "Replace or remove this marker before sharing the document.",
          replacement = "DONE"
        }
        start_at, end_at = line:find("TODO", end_at + 1, true)
      end
      line_number = line_number + 1
    end
    return findings
  end)
end

return extension
