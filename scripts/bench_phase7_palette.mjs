// Small synthetic command-palette filter benchmark; no app or extension processes are started.
const commands = Array.from({ length: 1000 }, (_, index) => `org.example.extension.command-${index}`);
const labels = new Map(commands.map((command, index) => [command, `Extension Command ${index}`]));
function filterCommands(query) {
  const normalized = query.trim().toLocaleLowerCase();
  return commands.filter((command) => command.toLocaleLowerCase().includes(normalized)
    || (labels.get(command) ?? '').toLocaleLowerCase().includes(normalized));
}

for (let warmup = 0; warmup < 1000; warmup += 1) filterCommands('command-99');
const samples = [];
for (let iteration = 0; iteration < 10_000; iteration += 1) {
  const start = performance.now();
  filterCommands('command-99');
  samples.push((performance.now() - start) * 1000);
}
const mean = samples.reduce((sum, value) => sum + value, 0) / samples.length;
console.log(`command_palette_filter_1000_commands,iterations=${samples.length},mean_us=${mean.toFixed(3)},min_us=${Math.min(...samples).toFixed(3)}`);
