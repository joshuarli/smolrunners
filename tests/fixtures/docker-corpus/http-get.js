const http = require('http');
const url = process.argv[2];
const expected = process.argv[3];
http.get(url, (res) => {
  let body = '';
  res.on('data', (chunk) => { body += chunk; });
  res.on('end', () => {
    const value = body.trim();
    console.log(value);
    process.exit(value.includes(expected) ? 0 : 1);
  });
}).on('error', (error) => {
  console.error(error.message);
  process.exit(1);
});
