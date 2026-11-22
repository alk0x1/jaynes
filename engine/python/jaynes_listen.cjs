'use strict';
// Jaynes's preload for node, in Jaynes's box only (NODE_OPTIONS=--require, added to the project's own): a server whose
// listen the box refuses (EPERM or EACCES) is said as the box's, and its process ends at once. Node's own test runner
// otherwise swallows the refusal and waits, so a suite hung until the check's time ran out, with nothing printed.
// Every other error of a listen, EADDRINUSE among them, goes on as node has it; where the box lets a server listen,
// nothing here runs.
const net = require('net');
const fs = require('fs');

const emit = net.Server.prototype.emit;
net.Server.prototype.emit = function (name, error, ...rest) {
  if (name === 'error' && error && error.syscall === 'listen' && (error.code === 'EPERM' || error.code === 'EACCES')) {
    // a socket file has its path; a port of the system's choosing (0) is left out of the error by node
    const address = String(error.address || 'localhost');
    const where = address.startsWith('/') ? address : `${address}:${error.port === undefined ? 0 : error.port}`;
    const line = `Jaynes's box refused listen on ${where}: operation not permitted`;
    try { fs.writeSync(2, `${line}\n`); } catch (_) { /* no stderr */ }
    const file = process.env.JAYNES_BOX_REFUSED;
    if (file) {
      try { fs.appendFileSync(file, `${line}\n`); } catch (_) { /* the box's folder is gone */ }
    }
    process.exit(1);
  }
  return emit.call(this, name, error, ...rest);
};
