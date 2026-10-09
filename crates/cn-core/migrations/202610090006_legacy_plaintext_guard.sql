-- #1705: 旧平文（#776 より前に暗号化せずに保存された通報者連絡先・申出者情報）の sealing を
-- cn-user-api の起動時に行わない。旧平文が残っていれば適用を止め、平文も列も消さない。止まった DB は、
-- #776 以後・#1705 より前の版の cn-user-api を一度起動して sealing してから適用し直す。
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM cn_admin.reports WHERE reporter_contact IS NOT NULL)
       OR EXISTS (
           SELECT 1 FROM cn_legal.rights_requests
           WHERE COALESCE(request_data->>'email', '') <> ''
       ) THEN
        RAISE EXCEPTION 'legacy plaintext legal data remains; start cn-user-api of a release between #776 and #1705 once to seal it, then apply this migration again';
    END IF;
END;
$$;

-- 通報者連絡先は暗号化した機微区分（cn_legal.sensitive_items）にだけ置く。
ALTER TABLE cn_admin.reports DROP COLUMN reporter_contact;
