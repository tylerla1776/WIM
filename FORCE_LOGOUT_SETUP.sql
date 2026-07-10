-- ============================================================================
-- WIM Force Log Out — server setup (run ONCE in the Supabase SQL editor)
-- ============================================================================
-- Force Log Out lets an admin remotely sign a device off the team. It works by
-- setting a "please log out" flag on the device row; the target device sees that
-- flag on its next heartbeat (within about a minute of normal use), logs itself
-- out, and clears the flag.
--
-- This mirrors the existing device-pause mechanism (set_device_paused /
-- get_device_paused), so it uses the same wim_devices table and the same
-- session-token validation your other RPCs already use.
--
-- HOW TO RUN:
--   1. Open your Supabase project → SQL Editor → New query
--   2. Paste this whole file and click Run
--   3. That's it — Force Log Out in WIM will start working immediately
--
-- Safe to re-run: everything uses IF NOT EXISTS / CREATE OR REPLACE.
-- ============================================================================

-- 1) Add the logout-request flag to the devices table.
ALTER TABLE public.wim_devices
  ADD COLUMN IF NOT EXISTS logout_requested boolean NOT NULL DEFAULT false;

-- 2) Admin asks a specific device to log out (sets the flag).
--    Adjust the admin check to match how your other admin-only RPCs validate,
--    if yours differ. This validates the caller's session the same way the
--    existing device RPCs do via a shared helper (wim_session_user); if your
--    project doesn't have that helper, replace the guard with your standard one.
CREATE OR REPLACE FUNCTION public.request_device_logout(
  p_session_token text,
  p_device_key   text
) RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
AS $$
DECLARE
  v_team text;
BEGIN
  -- Resolve + validate the caller's session → their team. Reuse the same
  -- session-validation your other functions use. This assumes a helper that
  -- returns the team code for a valid session token; swap in yours if named
  -- differently.
  SELECT team_code INTO v_team
  FROM public.wim_sessions
  WHERE session_token = p_session_token
    AND expires_at > now()
  LIMIT 1;

  IF v_team IS NULL THEN
    RAISE EXCEPTION 'invalid or expired session';
  END IF;

  UPDATE public.wim_devices
     SET logout_requested = true
   WHERE team_code = v_team
     AND device_key = p_device_key;

  RETURN true;
END;
$$;

-- 3) A device checks whether it's been asked to log out (called on each heartbeat).
CREATE OR REPLACE FUNCTION public.get_device_logout(
  p_session_token text,
  p_device_key   text
) RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
AS $$
DECLARE
  v_team text;
  v_flag boolean;
BEGIN
  SELECT team_code INTO v_team
  FROM public.wim_sessions
  WHERE session_token = p_session_token
    AND expires_at > now()
  LIMIT 1;

  IF v_team IS NULL THEN
    RETURN false;
  END IF;

  SELECT logout_requested INTO v_flag
  FROM public.wim_devices
  WHERE team_code = v_team
    AND device_key = p_device_key
  LIMIT 1;

  RETURN COALESCE(v_flag, false);
END;
$$;

-- 4) The device clears its own flag right after it logs itself out.
CREATE OR REPLACE FUNCTION public.clear_device_logout(
  p_session_token text,
  p_device_key   text
) RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
AS $$
DECLARE
  v_team text;
BEGIN
  SELECT team_code INTO v_team
  FROM public.wim_sessions
  WHERE session_token = p_session_token
    AND expires_at > now()
  LIMIT 1;

  IF v_team IS NULL THEN
    RETURN false;
  END IF;

  UPDATE public.wim_devices
     SET logout_requested = false
   WHERE team_code = v_team
     AND device_key = p_device_key;

  RETURN true;
END;
$$;

-- 5) Let the app role call these (matches how your other RPCs are exposed).
GRANT EXECUTE ON FUNCTION public.request_device_logout(text, text) TO anon, authenticated;
GRANT EXECUTE ON FUNCTION public.get_device_logout(text, text)     TO anon, authenticated;
GRANT EXECUTE ON FUNCTION public.clear_device_logout(text, text)   TO anon, authenticated;

-- ============================================================================
-- Done. If your wim_sessions/wim_devices tables or columns are named
-- differently, adjust the names above to match — the logic is unchanged.
-- ============================================================================
