// The page's script, in a file of its own: the page is served with a policy that allows scripts only
// from where it came from, so a script written into the document would not run.
//
// The parts a run names are in the fragment, after the `#`. A fragment is not part of a request, so
// a server, a proxy and a log between them never see the sender or the root. The browser's address
// bar and its history do hold the fragment until it is cleared, which is what the `replaceState`
// call below does once the parts are read.
//
//   index.html#relay=http://127.0.0.1:3340/&sender=<64 hex>&root=<64 hex>&size=2097152

const out = document.getElementById('out');
const parts = new URLSearchParams(location.hash.slice(1));
const hex = (bytes) =>
  [...new Uint8Array(bytes)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
const line = (text) => {
  out.textContent += text + '\n';
};
const started = Date.now();

try {
  if (!parts.get('relay') || !parts.get('sender') || !parts.get('root') || !parts.get('size')) {
    throw new Error('relay, sender, root and size are in the fragment, after the #');
  }
  // The parts are read, so the address bar and the history entry need them no longer. Replacing the
  // entry leaves the page where it is, and a window that is shown, shared or looked at later no
  // longer carries the sender or the root.
  history.replaceState(null, '', location.pathname + location.search);
  const module = await import('../sluice_wasm.js');
  await module.default();
  // The ticket's root is the hash sequence: the manifest root, then each file's root. This page
  // reads the one after the manifest, so it is the single-file case; a folder ticket needs the
  // manifest read to know which roots are files and what they are called. The hash sequence of a
  // one-file manifest is the manifest root and the file's root: 64 bytes.
  const sequence = new Uint8Array(
    await module.fetch_blob(parts.get('relay'), parts.get('sender'), parts.get('root'), 64),
  );
  line('sequence ' + sequence.length + ' bytes');
  const root = hex(sequence.slice(32));
  line('file root ' + root);
  // Every block of this one is verified against that root before it arrives here.
  const file = new Uint8Array(
    await module.fetch_blob(
      parts.get('relay'),
      parts.get('sender'),
      root,
      Number(parts.get('size')),
    ),
  );
  line(
    'file ' +
      file.length +
      ' bytes, sha256 ' +
      hex(await crypto.subtle.digest('SHA-256', file)) +
      ', ' +
      (Date.now() - started) +
      ' ms',
  );
  document.title = 'done';
} catch (e) {
  out.textContent = 'ERROR ' + String(e);
  document.title = 'failed';
}
