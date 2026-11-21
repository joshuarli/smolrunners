const net = require('net');
const port = Number(process.argv[2]);
const socket = net.connect(port, '127.0.0.1');
socket.on('connect', () => {
  socket.write('label=org.testcontainers=true\n');
  setTimeout(() => socket.end(), 2000);
});
socket.on('error', (error) => {
  console.error(error.message);
  process.exit(1);
});
socket.on('close', () => process.exit(0));
