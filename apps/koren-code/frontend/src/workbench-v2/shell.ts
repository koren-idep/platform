export const workbenchMarkup = `
  <div class="workbench-v2">
  <header class="wb-title-toolbar"><div class="menu-bar">
    <div class="brand" aria-label="Koren Code Editor"><span class="brand-mark">&lt;/&gt;</span><span class="brand-name">Koren Code Editor</span></div>
    <span class="brand-divider" aria-hidden="true"></span>
    <details class="wb-menu" open><summary aria-label="Main menu" title="Main menu"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3 4h10M3 8h10M3 12h10"/></svg></summary><nav class="menus" aria-label="Main menu">
      <details class="menu-group"><summary>File</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="open">Open Folder… <kbd>Ctrl+O</kbd></button>
        <button role="menuitem" data-action="open-file" id="menu-open-file" disabled>Open File… <kbd>Ctrl+Shift+O</kbd></button>
        <button role="menuitem" data-action="new-file">New File <kbd>Ctrl+N</kbd></button>
        <div class="menu-separator"></div>
        <button role="menuitem" data-action="save">Save <kbd>Ctrl+S</kbd></button>
        <button role="menuitem" data-action="save-all">Save All</button>
        <button role="menuitem" data-action="close-active">Close Active Tab <kbd>Ctrl+W</kbd></button>
      </div></details>
      <details class="menu-group"><summary>Edit</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="undo">Undo <kbd>Ctrl+Z</kbd></button>
        <button role="menuitem" data-action="redo">Redo <kbd>Ctrl+Y</kbd></button>
        <div class="menu-separator"></div><button role="menuitem" data-action="select-all">Select All <kbd>Ctrl+A</kbd></button>
      </div></details>
      <details class="menu-group"><summary>View</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="toggle-explorer">Toggle Project Explorer <kbd>Ctrl+B</kbd></button>
        <button role="menuitem" data-action="toggle-bottom">Toggle Bottom Panel <kbd>Ctrl+J</kbd></button>
        <div class="menu-separator"></div><button role="menuitem" data-action="toggle-theme">Toggle Theme <kbd>Alt+T</kbd></button>
        <div class="menu-separator"></div>
        <button role="menuitem" data-action="show-panel" data-panel-target="output">Output</button>
        <button role="menuitem" data-action="show-panel" data-panel-target="tasks">Tasks</button>
        <button role="menuitem" data-action="show-panel" data-panel-target="problems">Problems</button>
        <button role="menuitem" data-action="show-panel" data-panel-target="projects">Projects</button>
        <button role="menuitem" data-action="show-panel" data-panel-target="search">Search <kbd>Ctrl+Shift+F</kbd></button>
        <button role="menuitem" data-action="show-panel" data-panel-target="terminal">Terminal <kbd>Ctrl+grave</kbd></button>
        <button role="menuitem" data-action="source-control">Source Control</button>
        <div id="extension-panel-menu-items"></div>
        <div id="extension-wizard-menu-items"></div>
      </div></details>
      <details class="menu-group"><summary>Navigate</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="focus-explorer">Focus Project Explorer <kbd>Ctrl+B</kbd></button>
        <button role="menuitem" data-action="focus-editor">Focus Editor <kbd>F6</kbd></button>
        <button role="menuitem" data-action="go-to-definition">Go to Definition <kbd>F12</kbd></button>
        <div class="menu-separator"></div><button role="menuitem" data-action="previous-tab">Previous Tab <kbd>Ctrl+Shift+Tab</kbd></button>
        <button role="menuitem" data-action="next-tab">Next Tab <kbd>Ctrl+Tab</kbd></button>
        <div id="extension-navigate-items"></div>
      </div></details>
      <details class="menu-group"><summary>Run</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="discover">Discover Cargo Projects</button>
        <div class="menu-separator"></div>
        <button role="menuitem" data-action="run-cargo" data-command="build">Build Workspace</button>
        <button role="menuitem" data-action="run-cargo" data-command="check">Check Workspace</button>
        <button role="menuitem" data-action="run-cargo" data-command="test">Test Workspace</button>
        <button role="menuitem" data-action="run-cargo" data-command="clean">Clean Workspace</button>
        <div class="menu-separator"></div><button role="menuitem" data-action="cancel-task" disabled>Cancel Active Task <kbd>F5</kbd></button>
      </div></details>
      <details class="menu-group"><summary>Tools</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="command-palette">Command Palette… <kbd>Ctrl+Shift+P</kbd></button>
        <button role="menuitem" data-action="extensions">Extensions…</button>
        <button role="menuitem" data-action="import-sarif">Import SARIF Report…</button>
        <button role="menuitem" data-action="export-sarif">Export Current Inspections…</button>
        <div id="extension-tools-items"></div>
        <button role="menuitem" data-action="copy-path">Copy Current File Path</button>
        <button role="menuitem" data-action="refresh">Refresh Project Explorer</button>
      </div></details>
      <details class="menu-group"><summary>Help</summary><div class="menu-popup" role="menu">
        <button role="menuitem" data-action="shortcuts">Keyboard Shortcuts</button>
        <button role="menuitem" data-action="about">About Koren Code Editor</button>
      </div></details>
    </nav></details>
    <span class="wb-drag-region" aria-hidden="true"></span>
    <div class="window-controls" role="toolbar" aria-label="Window controls">
      <button type="button" data-window-action="minimize" title="Minimize" aria-label="Minimize"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3 8h10"/></svg></button>
      <button type="button" data-window-action="toggle_maximize" title="Maximize or restore" aria-label="Maximize or restore"><svg viewBox="0 0 16 16" aria-hidden="true"><rect x="3" y="3" width="10" height="10" rx=".6"/></svg></button>
      <button type="button" class="window-close" data-window-action="close" title="Close" aria-label="Close"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 4 8 8m0-8-8 8"/></svg></button>
    </div>
  </div>
  <div class="toolbar" role="toolbar" aria-label="Editor actions">
    <div class="toolbar-group wb-legacy-actions">
      <button class="toolbar-button" data-action="new-file" title="New File (Ctrl+N)" aria-label="New File"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 2.5v11M2.5 8h11"/></svg></button>
      <button class="toolbar-button" data-action="open" title="Open Folder (Ctrl+O)" aria-label="Open Folder"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M1.8 4.5h5l1.5 1.7h5.9v6.2H1.8zM1.8 4.5V3h4l1.4 1.5"/></svg></button>
      <button class="toolbar-button" data-action="save" id="toolbar-save" disabled title="Save (Ctrl+S)" aria-label="Save"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 2h10l2 2v10H2zM5 2v4h6V2M5 14V9h6v5"/></svg></button>
    </div>
    <span class="toolbar-divider" aria-hidden="true"></span>
    <div class="toolbar-group wb-legacy-actions">
      <button class="toolbar-button" data-action="undo" id="toolbar-undo" disabled title="Undo (Ctrl+Z)" aria-label="Undo"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 5 1.8 7.2 4 9.4M2 7.2h6.2a4 4 0 0 1 0 8" transform="translate(0 -2)"/></svg></button>
      <button class="toolbar-button" data-action="redo" id="toolbar-redo" disabled title="Redo (Ctrl+Y)" aria-label="Redo"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="m12 5 2.2 2.2L12 9.4M14 7.2H7.8a4 4 0 0 0 0 8" transform="translate(0 -2)"/></svg></button>
    </div>
    <span class="toolbar-divider" aria-hidden="true"></span>
    <button class="toolbar-button" data-action="source-control" title="Source Control" aria-label="Source Control"><svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="4" cy="3" r="1.5"/><circle cx="4" cy="13" r="1.5"/><circle cx="12" cy="5" r="1.5"/><path d="M4 4.5v7M4 8h3a5 5 0 0 0 5-5"/></svg></button>
    <div class="run-controls">
      <button class="run-button" data-action="run-selected" id="toolbar-run" disabled title="Run selected configuration after approval"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M5 2.5 13 8l-8 5.5z"/></svg></button>
      <select id="run-configuration" aria-label="Run configuration" disabled><option value="">No run configuration</option></select>
    </div>
    <span class="toolbar-spacer"></span><span id="toolbar-workspace" class="wb-workspace-name">Koren Code Editor</span><span class="toolbar-spacer"></span>
    <button class="toolbar-button toolbar-utility" data-action="command-palette" title="Command Palette (Ctrl+Shift+P)" aria-label="Command Palette"><svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5"/><path d="m10.4 10.4 3 3"/></svg></button>
    <button class="toolbar-button toolbar-utility" data-action="toggle-secondary" title="Toggle secondary sidebar" aria-label="Toggle secondary sidebar"><svg viewBox="0 0 16 16" aria-hidden="true"><rect x="2" y="2.5" width="12" height="11" rx="1"/><path d="M10 3v10"/></svg></button>
    <button class="toolbar-button toolbar-utility" data-action="settings" title="Settings" aria-label="Settings"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 1.6 13.4 4.7v6.6L8 14.4l-5.4-3.1V4.7z"/><circle cx="8" cy="8" r="2.1"/></svg></button>
  </div></header>
  <main class="workbench" id="workbench">
    <aside class="explorer" id="explorer">
      <div class="panel-heading"><span id="primary-sidebar-title">FILES</span><span class="panel-actions"><button data-action="refresh" title="Refresh project view" aria-label="Refresh project view">↻</button><details class="panel-options"><summary title="Project panel options" aria-label="Project panel options">⋮</summary><div class="panel-options-menu" role="menu"><button role="menuitem" data-action="toggle-explorer">Collapse Project Panel</button></div></details><button data-action="toggle-explorer" title="Collapse explorer" aria-label="Collapse explorer">‹</button></span></div>
      <nav id="primary-sidebar-tabs" class="primary-sidebar-tabs" role="tablist" aria-label="Project views"><button type="button" data-sidebar-view="files" role="tab" aria-selected="true">Files</button><button type="button" data-sidebar-view="search" role="tab" aria-selected="false">Search</button><button type="button" data-sidebar-view="git" role="tab" aria-selected="false">Git</button><button type="button" data-sidebar-view="extensions" role="tab" aria-selected="false">Extensions</button></nav>
      <div class="primary-sidebar-content"><div id="tree" class="tree" role="tree" aria-label="Project files"></div><div id="search" class="panel-content developer-panel sidebar-tool-view hidden"><form id="search-form" class="developer-controls"><input id="search-query" type="search" placeholder="Search workspace" aria-label="Search workspace"><input id="search-filter" type="text" placeholder="Files to include (glob)" aria-label="File name filter"><label><input id="search-case" type="checkbox"> Match case</label><label><input id="search-regex" type="checkbox"> Regex</label><button type="submit">Search</button><button type="button" data-action="cancel-search" disabled>Cancel</button></form><div id="search-status" class="subtle">Search starts only when requested. Results are limited to 5,000.</div><div id="search-results" class="developer-results"></div></div><div id="source-control" class="panel-content developer-panel sidebar-tool-view hidden"><div class="developer-controls"><select id="git-repository" aria-label="Git repository"><option value="">No repository selected</option></select><button data-action="refresh-git">↻ Refresh</button></div><div id="git-branch" class="subtle"></div><div class="git-columns"><section><h3>Changes</h3><div id="git-unstaged" class="developer-results"></div></section><section><h3>Staged</h3><div id="git-staged" class="developer-results"></div></section></div><pre id="git-diff" class="git-diff" aria-label="Git diff preview"></pre><form id="git-commit-form" class="developer-controls"><input id="git-commit-message" type="text" placeholder="Commit message" aria-label="Commit message"><button type="submit">Commit staged</button></form></div><div id="sidebar-extensions" class="sidebar-extension-view hidden"></div></div>
      <div class="explorer-footer"><button data-action="discover">Discover Cargo</button></div>
    </aside>
    <div id="explorer-resizer" class="explorer-resizer" role="separator" aria-label="Resize project explorer" aria-orientation="vertical" tabindex="0"></div>
    <section class="main-column">
      <section class="editor-frame" aria-label="Editor workspace">
        <div id="tabs" class="tabs" role="tablist" aria-label="Open files"></div>
        <div id="editor" class="editor hidden" aria-label="Code editor"></div>
        <section id="welcome" class="welcome">
          <div class="welcome-mark">&lt;/&gt;</div><div class="welcome-title">Koren Code Editor</div>
          <p>A focused workspace for your code.</p>
          <div class="welcome-actions"><button class="primary-button" data-action="open">Open Folder</button><button data-action="open-file" id="welcome-open-file">Open File</button></div>
          <div class="welcome-hint">Open a folder <kbd>Ctrl+O</kbd> <span>·</span> Open a file <kbd>Ctrl+Shift+O</kbd> <span>·</span> Command Palette <kbd>Ctrl+Shift+P</kbd></div>
        </section>
      </section>
  <div class="wb-bottom-area">
      <div class="bottom-resizer" id="bottom-resizer" role="separator" aria-label="Resize bottom panel" aria-orientation="horizontal" tabindex="0"></div>
      <section class="bottom-panel" id="bottom-panel">
        <div class="bottom-heading"><div class="tool-tabs" role="tablist" aria-label="Tool panels">
          <button class="selected" data-panel="output" role="tab" aria-selected="true">Output</button><button data-panel="problems" role="tab" aria-selected="false">Problems <span id="problem-count" class="count-badge">0</span></button><button data-panel="tasks" role="tab" aria-selected="false">Tasks</button><button data-panel="projects" role="tab" aria-selected="false">Projects</button><button data-panel="terminal" role="tab" aria-selected="false">Terminal</button><button data-panel="debug" role="tab" aria-selected="false">Debug</button>
        </div><span id="bottom-task-state" class="task-state"></span><span class="panel-actions"><button data-action="cancel-task" title="Cancel active task" aria-label="Cancel active task" disabled>■</button><button data-action="toggle-bottom" title="Collapse bottom panel" aria-label="Collapse bottom panel">⌄</button></span></div>
        <div id="output" class="panel-content output"><div class="panel-empty">Task output will appear here.</div></div>
        <div id="problems" class="panel-content problem-list hidden"><div class="problem-filters"><label>Severity <select id="problem-severity-filter"><option value="all">All</option><option value="error">Errors</option><option value="warning">Warnings</option><option value="information">Information</option><option value="hint">Hints</option></select></label><label>Group by <select id="problem-grouping"><option value="file">File</option><option value="provider">Provider</option><option value="category">Category</option><option value="severity">Severity</option></select></label></div><div id="problem-entries"><div class="panel-empty">No problems reported by installed language integrations.</div></div></div>
        <div id="tasks" class="panel-content task-list hidden"><div class="panel-empty">Discover Cargo projects to see available tasks.</div></div>
        <div id="projects" class="panel-content project-list hidden"><div class="project-analysis-controls"><button data-action="start-project-analysis">Analyze Open Documents</button><button data-action="cancel-project-analysis" disabled>Cancel</button><button data-action="load-project-findings" hidden>Load More Findings</button><span id="project-analysis-status">Uses a bounded snapshot of currently open documents.</span></div><div id="project-list"><div class="panel-empty">Project information will appear here.</div></div></div>
        <div id="terminal" class="panel-content terminal-panel hidden"><div class="terminal-toolbar"><button type="button" data-terminal-new>＋ New</button><button type="button" data-terminal-rename disabled>Rename</button><button type="button" data-terminal-restart disabled>Restart</button><button type="button" data-terminal-close disabled>Close</button><span data-terminal-status class="subtle">No terminal sessions.</span></div><div class="terminal-session-tabs" data-terminal-tabs role="tablist" aria-label="Terminal sessions"></div><div class="terminal-session-views" data-terminal-views></div><div class="terminal-shortcut-hint">Ctrl+Shift+C copy · Ctrl+Shift+V paste · Ctrl+L clear display</div></div>
        <div id="debug" class="panel-content debug-panel hidden">
          <div class="debug-toolbar"><select data-debug-configuration aria-label="Debug configuration"></select><button type="button" data-debug-start disabled>Start</button><button type="button" data-debug-continue disabled>Continue</button><button type="button" data-debug-pause disabled>Pause</button><button type="button" data-debug-step-over disabled>Step Over</button><button type="button" data-debug-step-into disabled>Step Into</button><button type="button" data-debug-step-out disabled>Step Out</button><button type="button" data-debug-stop disabled>Stop</button><select data-debug-session aria-label="Active debug session"></select><button type="button" data-debug-refresh title="Refresh sessions">↻</button><span data-debug-state class="subtle">No active debug session</span></div>
          <details data-debug-config-editor class="debug-config-editor"><summary>Configure local DAP profile</summary><form data-debug-config-form class="debug-config-form"><label>Adapter ID <input name="adapter_id" required placeholder="local.dap"></label><label>Adapter name <input name="adapter_name" placeholder="Debug Adapter"></label><label>Adapter executable <input name="adapter_command" required placeholder="/path/to/adapter"></label><label>Adapter arguments <input name="adapter_arguments" placeholder="--stdio"></label><label>Adapter working directory <input name="adapter_directory" required placeholder="/workspace"></label><label>Configuration ID <input name="configuration_id" required placeholder="app.launch"></label><label>Configuration name <input name="configuration_name" placeholder="Launch application"></label><label>Program <input name="program" required placeholder="/workspace/target/debug/app"></label><label>Program arguments <input name="arguments" placeholder="--help"></label><label>Target working directory <input name="working_directory" required placeholder="/workspace"></label><button class="primary" type="submit">Save profile</button><span class="subtle">Saved profiles never start until you review and approve each exact launch.</span></form></details>
          <div class="debug-columns"><section><h3>Threads</h3><div data-debug-threads></div><h3>Call Stack</h3><div data-debug-stack></div></section><section><h3>Variables</h3><div data-debug-variables></div><h3>Breakpoints</h3><div data-debug-breakpoints></div><h3>Debug Output</h3><div data-debug-output class="debug-output" role="log" aria-live="polite"></div></section></div>
        </div>
        <div id="extension-panel-content"></div>
      </section>
  </div>
    </section>
    <div id="secondary-resizer" class="secondary-resizer" role="separator" aria-label="Resize secondary sidebar" aria-orientation="vertical" tabindex="0"></div>
    <aside id="secondary-sidebar" class="secondary-sidebar" aria-label="Secondary tools"><div class="secondary-heading"><span>OUTLINE</span><button data-action="toggle-secondary" aria-label="Close secondary sidebar" title="Close secondary sidebar">×</button></div><div class="secondary-content"><p>Document outline will appear when a language integration provides symbols.</p><button data-action="extensions">Manage Extensions…</button></div></aside>
  </main>

  <footer class="status-bar">
    <div class="status-workspace"><span class="status-workspace-icon" aria-hidden="true">▱</span><span>Workspace:</span><strong id="status-workspace">No Workspace</strong></div>
    <span id="status-message" class="status-message">Ready</span><span id="status-document" class="status-revision"></span><span class="status-spacer"></span>
    <span id="status-language" class="status-item"></span><span id="status-encoding" class="status-item"></span><span id="status-line-ending" class="status-item"></span><span id="status-cursor" class="status-item"></span>
  </footer>
  <div class="wb-overlay-host">
  <dialog id="approval-dialog" class="dialog"><form method="dialog"><header>Run task?</header><p class="subtle">Review the command before it starts. Approval is not a sandbox.</p><dl id="approval-details"></dl><div class="dialog-actions"><button value="deny" class="secondary">Deny</button><button value="approve" class="primary">Approve once</button></div></form></dialog>
  <dialog id="conflict-dialog" class="dialog conflict"><form method="dialog"><header>File changed outside Koren</header><p id="conflict-path"></p><details><summary>Inspect current disk contents</summary><pre id="conflict-preview"></pre></details><p class="subtle">Choose whether to reload the external version or overwrite it with your buffer. If the file changes again, Koren will ask again.</p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="reload" class="secondary">Reload disk version</button><button value="overwrite" class="primary">Overwrite file</button></div></form></dialog>
  <dialog id="confirm-dialog" class="dialog"><form method="dialog"><header id="confirm-title">Unsaved changes</header><p id="confirm-message"></p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="discard" class="primary">Discard changes</button></div></form></dialog>
  <dialog id="recovery-dialog" class="dialog"><header>Recover unsaved edits?</header><p>Choose which drafts to restore. Changed or missing files are restored to a new workspace file so their current contents are never replaced.</p><div id="recovery-items" class="recovery-items"></div><div class="dialog-actions"><button type="button" class="secondary" data-action="close-recovery">Later</button></div></dialog>
  <dialog id="close-dialog" class="dialog"><form method="dialog"><header>Unsaved documents</header><p>Save your changes before closing Koren Code Editor?</p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="discard" class="secondary">Discard and close</button><button value="save" class="primary">Save all and close</button></div></form></dialog>
  <dialog id="switch-workspace-dialog" class="dialog"><form method="dialog"><header>Switch workspace?</header><p>Choose what to do with open documents before changing folders.</p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="discard" class="secondary">Discard and switch</button><button value="save" class="primary">Save and switch</button></div></form></dialog>
  <dialog id="command-dialog" class="dialog command-dialog"><form method="dialog"><header>Command Palette</header><input id="command-search" type="search" placeholder="Search available commands" autocomplete="off" aria-label="Search commands"><div id="command-results" role="listbox" aria-label="Available commands"></div><footer><kbd>↑</kbd><kbd>↓</kbd> navigate <kbd>Enter</kbd> run <kbd>Esc</kbd> close</footer></form></dialog>
  <dialog id="extensions-dialog" class="dialog extensions-dialog"><form method="dialog"><header>Extensions</header><p class="subtle">Local extensions from the configured Koren extensions folder. Code runs only after you enable and activate it.</p><div id="extension-list" class="extension-list"></div><div class="dialog-actions"><button value="close" class="primary">Close</button></div></form></dialog>
  <dialog id="language-server-approval-dialog" class="dialog"><form method="dialog"><header>Start language server?</header><p class="subtle">This starts an external program with your user account permissions. Approval is not a sandbox. Review the exact command and workspace.</p><dl id="language-server-approval-details"></dl><div class="dialog-actions"><button value="deny" class="secondary">Deny</button><button value="approve" class="primary">Approve and start</button></div></form></dialog>
  <dialog id="terminal-approval-dialog" class="dialog"><form><header>Start interactive shell?</header><p class="subtle">Approval grants this shell interactive access using your account permissions. Commands entered later are not individually reviewed, and this is not a filesystem sandbox.</p><dl data-terminal-approval-details></dl><div class="dialog-actions"><button type="submit" value="deny" class="secondary">Deny</button><button type="submit" value="approve" class="primary">Approve and start</button></div></form></dialog>
  <dialog id="debug-approval-dialog" class="dialog"><form><header>Authorize debugger and target?</header><p class="subtle">The adapter can inspect the target process. Approval is not a sandbox. Environment values are hidden.</p><dl data-debug-approval-details></dl><div class="dialog-actions"><button type="submit" value="deny" class="secondary">Deny</button><button type="submit" value="approve" class="primary">Approve once</button></div></form></dialog>
  <dialog id="settings-dialog" class="dialog settings-dialog">
    <section class="settings-window" aria-label="Application settings">
      <header class="settings-header"><div><h1>Settings</h1><p>Configure the editor and workspace.</p></div><form method="dialog"><button class="settings-close" aria-label="Close Settings">×</button></form></header>
      <div class="settings-toolbar"><input id="settings-search" type="search" placeholder="Search settings by name or ID" aria-label="Search settings"><label>Scope <select id="settings-scope"><option value="user">User</option><option value="workspace">Workspace</option></select></label><label class="settings-modified"><input id="settings-modified" type="checkbox"> Modified</label></div>
      <div id="settings-warning" class="settings-warning hidden" role="alert"></div>
      <div class="settings-layout"><nav id="settings-categories" class="settings-categories" aria-label="Settings categories"></nav><main id="settings-content" class="settings-content"><div id="settings-builtins"></div><section id="settings-extension-section" class="settings-section hidden"><div id="extension-settings" class="extension-settings"></div></section><section id="settings-inspection-section" class="settings-section hidden"><div id="inspection-settings" class="inspection-settings"></div></section></main></div>
      <footer class="settings-footer"><span id="settings-storage-status" role="status">Settings are saved automatically.</span><div><button id="settings-reset-category" type="button">Restore Category Defaults</button><button id="settings-reset-all" type="button">Reset All Settings</button><button id="settings-open-file" type="button" disabled title="Opening the configuration file is not available yet">Open Settings File</button></div></footer>
    </section>
  </dialog>
  <dialog id="info-dialog" class="dialog"><form method="dialog"><header id="info-title"></header><div id="info-content" class="info-content"></div><div class="dialog-actions"><button value="close" class="primary">Close</button></div></form></dialog>
  <dialog id="workspace-input-dialog" class="dialog"><form id="workspace-input-form"><header id="workspace-input-title"></header><label id="workspace-input-label" for="workspace-input"></label><input id="workspace-input" type="text" autocomplete="off"><div class="dialog-actions"><button type="button" value="cancel" class="secondary">Cancel</button><button type="submit" class="primary">Continue</button></div></form></dialog>
  <dialog id="workspace-delete-dialog" class="dialog"><form method="dialog"><header>Delete workspace entry?</header><p id="workspace-delete-path"></p><p class="subtle">This permanently deletes the selected entry from disk. Open dirty documents are protected.</p><div class="dialog-actions"><button value="cancel" class="secondary">Cancel</button><button value="delete" class="danger">Delete</button></div></form></dialog>
  <dialog id="project-wizard-dialog" class="dialog"><form id="project-wizard-form"><header id="project-wizard-title"></header><div id="project-wizard-fields" class="extension-panel-view"></div><p class="subtle">Submitting validates these fields only. Project creation must be performed by a host-authorized extension operation.</p><div class="dialog-actions"><button type="button" class="secondary" data-action="close-wizard">Cancel</button><button type="submit" class="primary">Validate form</button></div></form></dialog>
  <div id="status-toast" role="status" aria-live="polite"></div>
  </div>
  </div>`;
