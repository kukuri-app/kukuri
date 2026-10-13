import { useEffect, useState } from 'react';

import type { GameRoomView, MetaverseAssetRef } from '@/lib/api';
import type { MetaverseRoomActions } from './MetaverseRoomActions';
import type { DomeConnectionsState } from './useDomeConnections';
import {
  resolveActiveDomeNeighbors,
  type DomeNeighborTransitionView,
} from './DomeTransitionModel';

export function useDomeTransitionNeighbors(
  actions: MetaverseRoomActions,
  selectedRoom: GameRoomView | null,
  rooms: GameRoomView[],
  participantPubkey: string,
  connections: DomeConnectionsState
) {
  const state = useState<DomeNeighborTransitionView[]>([]);
  const [, setNeighbors] = state;
  const readFailed = connections.status === 'error';

  useEffect(() => {
    if (!selectedRoom?.metaverse) {
      setNeighbors([]);
      return;
    }
    let cancelled = false;
    const load = async () => {
      try {
        const topology = connections.topology;
        if (!topology) { setNeighbors(current => current.length ? [] : current); return; }
        if (readFailed) {
          setNeighbors(current => current.map(neighbor => ({ ...neighbor, boundaryState: 'error' })));
          return;
        }
        const loading = resolveActiveDomeNeighbors(topology, selectedRoom, rooms, {}, {});
        if (cancelled) return;
        // 一覧の読み直しのたびに確かめ直す。その間も、同じ接続の同じ相手は確かめ済みの状態を保つ(空間音声・presence が
        // 通れる隣を数秒ごとに外さない)。
        setNeighbors(current => loading.length || current.length
          ? loading.map(neighbor => current.find(previous => previous.connectionId === neighbor.connectionId
            && previous.room.room_id === neighbor.room.room_id) ?? neighbor)
          : current);
        const hostingErrors = new Set<string>();
        const hostingEntries = await Promise.all(loading.map(async (neighbor) => {
          try {
            return [neighbor.room.metaverse!.instance_id, await actions.getHosting(
              neighbor.room.metaverse!.spatial_context,
              neighbor.room.metaverse!.instance_id
            )] as const;
          } catch {
            hostingErrors.add(neighbor.room.metaverse!.instance_id);
            return [neighbor.room.metaverse!.instance_id, undefined] as const;
          }
        }));
        const hosting = Object.fromEntries(hostingEntries);
        if (cancelled) return;
        const accessEntries = await Promise.all(loading.map(async (neighbor) => {
          try {
            const decision = await actions.previewTransitionAccess({
              transition_id: `preview-${neighbor.connectionId}`,
              connection_id: neighbor.connectionId,
              topology_digest: neighbor.topologyDigest,
              spatial_context: selectedRoom.metaverse!.spatial_context,
              source_instance_id: selectedRoom.metaverse!.instance_id,
              source_instance_generation: selectedRoom.metaverse!.instance_generation,
              target_instance_id: neighbor.room.metaverse!.instance_id,
              target_instance_generation: neighbor.room.metaverse!.instance_generation,
              participant_pubkey: participantPubkey,
              direction: neighbor.direction,
              requested_at: Date.now(),
            });
            return [neighbor.connectionId, decision.status === 'allowed' ? 'allowed' : 'denied'] as const;
          } catch {
            return [neighbor.connectionId, 'error'] as const;
          }
        }));
        const accessByConnection = Object.fromEntries(accessEntries);
        if (cancelled) return;
        const assetStates: Record<string, 'loading' | 'ready' | 'error'> = {};
        const textureUrls: Record<string, { wall: string | null; floor: string | null }> = {};
        await Promise.all(loading.map(async (neighbor) => {
          if (cancelled || accessByConnection[neighbor.connectionId] !== 'allowed') return;
          const metaverse = neighbor.room.metaverse!;
          const refs = [
            ...metaverse.asset_refs,
            metaverse.dome.customization.surface.wall_texture,
            metaverse.dome.customization.surface.floor_texture,
            ...metaverse.dome.customization.persistent_props.map((prop) => prop.asset_ref),
          ].filter((asset): asset is MetaverseAssetRef => Boolean(asset));
          const unique = [...new Map(refs.map((asset) => [asset.blob_hash, asset])).values()];
          try {
            const resolved = new Map<string, string | null>();
            await Promise.all(unique.map(async (asset) => {
              resolved.set(asset.blob_hash, await actions.getBlobPreviewUrl(
                asset.blob_hash,
                asset.mime_type ?? 'application/octet-stream',
                asset.kind
              ));
            }));
            if (unique.some((asset) => !resolved.get(asset.blob_hash))) {
              throw new Error('neighbor Dome asset is unavailable');
            }
            textureUrls[metaverse.instance_id] = {
              wall: metaverse.dome.customization.surface.wall_texture
                ? resolved.get(metaverse.dome.customization.surface.wall_texture.blob_hash) ?? null
                : null,
              floor: metaverse.dome.customization.surface.floor_texture
                ? resolved.get(metaverse.dome.customization.surface.floor_texture.blob_hash) ?? null
                : null,
            };
            assetStates[metaverse.instance_id] = 'ready';
          } catch {
            assetStates[metaverse.instance_id] = 'error';
          }
        }));
        if (!cancelled) {
          const resolved: DomeNeighborTransitionView[] = resolveActiveDomeNeighbors(
            topology,
            selectedRoom,
            rooms,
            hosting,
            assetStates,
            textureUrls
          ).map((neighbor): DomeNeighborTransitionView => {
            if (['draining', 'blocked', 'closed'].includes(neighbor.boundaryState)) return neighbor;
            if (hostingErrors.has(neighbor.room.metaverse!.instance_id) || accessByConnection[neighbor.connectionId] === 'error')
              return { ...neighbor, boundaryState: 'error' };
            return accessByConnection[neighbor.connectionId] === 'allowed' ? neighbor : { ...neighbor, boundaryState: 'closed' };
          });
          setNeighbors(current => resolved.length || current.length ? resolved : current);
        }
      } catch {
        if (!cancelled) {
          setNeighbors((current) => current.map((neighbor) => ({
            ...neighbor,
            boundaryState: 'error',
          })));
        }
      }
    };
    void load();
    return () => {
      cancelled = true;
    };
  }, [actions, participantPubkey, rooms, selectedRoom, setNeighbors, connections.topology, readFailed]);

  return state;
}
