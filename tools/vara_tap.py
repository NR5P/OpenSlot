#!/usr/bin/env python3
"""Logging TCP proxy for the VARA host interface (command, data and KISS ports).

Sits between a VARA client (VarAC, Openslot, ...) and a VARA-compatible
modem (VARA, XARA) and records every byte in both directions, so the
on-the-wire behaviour of real VarAC can be compared with ours.

    vara_tap.py --name A --listen 8300 --kiss-listen 8100 \
                --modem 9300 --kiss-modem 9100 --log a.log

Data port = command port + 1 on both sides. Log lines:
    <time> <name> <chan> <dir> <repr of bytes>
where chan is cmd|data|kiss and dir is '>' (client->modem) or '<'.
"""
import argparse
import asyncio
import time


def fmt(b: bytes) -> str:
    return repr(b)[2:-1]


async def pump(reader, writer, log, name, chan, arrow):
    try:
        while True:
            data = await reader.read(65536)
            if not data:
                break
            log.write(f"{time.strftime('%H:%M:%S')}.{int(time.time() * 1000) % 1000:03d} {name} {chan:4} {arrow} {fmt(data)}\n")
            log.flush()
            writer.write(data)
            await writer.drain()
    except (ConnectionError, asyncio.CancelledError):
        pass
    finally:
        try:
            writer.close()
        except Exception:
            pass


MODEM_CMD_WRITERS = []


async def inject_server(port, log, name):
    """Lines sent to this port are written to the modem's command port as if the
    client had sent them (e.g. to start a CONNECT without clicking in the client)."""
    async def handle(r, w):
        while line := await r.readline():
            cmd = line.strip() + b"\r"
            log.write(f"{time.strftime('%H:%M:%S')} {name} cmd  I {fmt(cmd)}\n")
            log.flush()
            for mw in MODEM_CMD_WRITERS:
                mw.write(cmd)
                await mw.drain()
        w.close()
    return await asyncio.start_server(handle, "127.0.0.1", port)


def make_handler(host, port, log, name, chan):
    async def handle(c_reader, c_writer):
        log.write(f"{time.strftime('%H:%M:%S')} {name} {chan:4} * client connected\n")
        log.flush()
        try:
            m_reader, m_writer = await asyncio.open_connection(host, port)
        except OSError as e:
            log.write(f"{time.strftime('%H:%M:%S')} {name} {chan:4} ! modem connect failed: {e}\n")
            log.flush()
            c_writer.close()
            return
        if chan == "cmd":
            MODEM_CMD_WRITERS.clear()
            MODEM_CMD_WRITERS.append(m_writer)
        await asyncio.gather(
            pump(c_reader, m_writer, log, name, chan, ">"),
            pump(m_reader, c_writer, log, name, chan, "<"),
        )
        log.write(f"{time.strftime('%H:%M:%S')} {name} {chan:4} * closed\n")
        log.flush()
    return handle


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", required=True)
    ap.add_argument("--listen", type=int, required=True, help="client-facing command port (data = +1)")
    ap.add_argument("--kiss-listen", type=int)
    ap.add_argument("--modem-host", default="127.0.0.1")
    ap.add_argument("--modem", type=int, required=True, help="modem command port (data = +1)")
    ap.add_argument("--kiss-modem", type=int)
    ap.add_argument("--log", required=True)
    ap.add_argument("--inject", type=int, help="port accepting command lines to inject toward the modem")
    a = ap.parse_args()
    log = open(a.log, "a", buffering=1)
    pairs = [("cmd", a.listen, a.modem), ("data", a.listen + 1, a.modem + 1)]
    if a.kiss_listen and a.kiss_modem:
        pairs.append(("kiss", a.kiss_listen, a.kiss_modem))
    servers = []
    for chan, lp, mp in pairs:
        servers.append(await asyncio.start_server(make_handler(a.modem_host, mp, log, a.name, chan), "127.0.0.1", lp))
    if a.inject:
        servers.append(await inject_server(a.inject, log, a.name))
    log.write(f"{time.strftime('%H:%M:%S')} {a.name} tap up: {pairs}\n")
    await asyncio.gather(*(s.serve_forever() for s in servers))


if __name__ == "__main__":
    asyncio.run(main())
