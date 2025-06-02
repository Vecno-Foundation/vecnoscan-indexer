# encoding: utf-8
import asyncio

from vecnod.VecnodThread import VecnodThread, VecnodCommunicationError
import logging

_logger = logging.getLogger(__name__)

# pipenv run python -m grpc_tools.protoc -I./protos --python_out=. --grpc_python_out=. ./protos/rpc.proto ./protos/messages.proto ./protos/p2p.proto

class VecnodClient(object):
    def __init__(self, vecnod_host, vecnod_port):
        self.vecnod_host = vecnod_host
        self.vecnod_port = vecnod_port
        self.server_version = None
        self.is_utxo_indexed = None
        self.is_synced = None
        self.p2p_id = None

    async def ping(self):
        try:
            info = await self.request("getInfoRequest")
            self.server_version = info["getInfoResponse"]["serverVersion"]
            self.is_utxo_indexed = info["getInfoResponse"]["isUtxoIndexed"]
            self.is_synced = info["getInfoResponse"]["isSynced"]
            self.p2p_id = info["getInfoResponse"]["p2pId"]
            return info

        except Exception as exc:
            return False

    async def request(self, command, params=None, timeout=60, retry=0):
        for i in range(1 + retry):
            try:
                with VecnodThread(self.vecnod_host, self.vecnod_port) as t:
                    resp = await t.request(command, params, wait_for_response=True, timeout=timeout)
                    return resp
            except VecnodCommunicationError:
                if i == retry:
                    raise
                else:
                    await asyncio.sleep(0.3)
            except Exception:
                raise

    async def notify(self, command, params, callback):
        t = VecnodThread(self.vecnod_host, self.vecnod_port, async_thread=True)
        return await t.notify(command, params, callback)
