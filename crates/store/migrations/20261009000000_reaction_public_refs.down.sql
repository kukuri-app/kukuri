DELETE FROM public_blob_ref_backfill WHERE kind IN ('reaction_asset', 'reaction_bookmark');
DELETE FROM public_blob_refs WHERE source_kind IN ('reaction_asset', 'reaction_bookmark');
DELETE FROM public_blob_announcements
WHERE blob_hash NOT IN (SELECT blob_hash FROM public_blob_refs);
