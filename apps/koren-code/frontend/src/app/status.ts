export function formatCursorPosition(line: number, column: number): string {
  return `Ln ${line}, Col ${column}`;
}

export function lineEndingFor(contents: string): 'LF' | 'CRLF' {
  return contents.includes('\r\n') ? 'CRLF' : 'LF';
}
