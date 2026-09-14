import http from "node:http";

let position = 50;

const headers = {
  "Content-Type": "application/json",
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
  "Access-Control-Allow-Headers": "Content-Type",
};

http
  .createServer((req, res) => {
    if (req.method === "OPTIONS") return res.writeHead(204, headers).end();
    if (req.url !== "/position") return res.writeHead(404, headers).end();

    if (req.method === "GET") {
      return res.writeHead(200, headers).end(JSON.stringify({ position }));
    }

    if (req.method === "POST") {
      let body = "";
      req.on("data", (chunk) => (body += chunk));
      req.on("end", () => {
        const value = Number(JSON.parse(body || "{}").position);
        if (Number.isNaN(value)) return res.writeHead(400, headers).end();
        position = Math.min(100, Math.max(0, value));
        res.writeHead(200, headers).end(JSON.stringify({ position }));
      });
      return;
    }

    res.writeHead(405, headers).end();
  })
  .listen(3001, () => console.log("backend http://localhost:3001"));
