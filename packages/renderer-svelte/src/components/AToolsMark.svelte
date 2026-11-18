<script lang="ts">
  type Props = {
    size?: "small" | "badge" | "large";
    label?: string;
    circular?: boolean;
    streaming?: boolean;
  };

  let { size = "badge", label = "", circular = false, streaming = false }: Props = $props();
</script>

<span
  class={`atools-mark ${size}`}
  class:circular
  class:streaming
  aria-hidden={!label}
  aria-label={label || undefined}
  role={label ? "img" : undefined}
>
  <svg viewBox="0 0 128 128" aria-hidden="true" focusable="false">
    <path d="M25 99 55 22c1.7-4.4 3.8-6.5 8.1-6.5 4.1 0 6.4 2.1 8.1 6.5L103 99H82L75 80H53l-7 19H25Zm34-36h10l-5-14-5 14Z" />
  </svg>
  {#if streaming}
    <span class="ai-ring" aria-hidden="true"></span>
  {/if}
</span>

<style>
  .atools-mark {
    --mark-size: 36px;
    --mark-radius: var(--radius-md);
    position: relative;
    width: var(--mark-size);
    height: var(--mark-size);
    flex: 0 0 var(--mark-size);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    overflow: visible;
    border-radius: var(--mark-radius);
    background-color: #047857;
    background-image: linear-gradient(135deg, #047857 0%, #059669 55%, #10b981 100%);
    box-shadow:
      0 2px 8px -3px rgba(4, 120, 87, 0.45),
      inset 0 0 0 1px rgba(255, 255, 255, 0.2);
  }

  .atools-mark.small {
    --mark-size: 20px;
    --mark-radius: var(--radius-sm);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.2);
  }

  .atools-mark.large {
    --mark-size: 72px;
    --mark-radius: 16px;
  }

  .atools-mark.circular {
    --mark-radius: 50%;
  }

  svg {
    display: block;
    width: 64%;
    height: 64%;
    fill: #ffffff;
  }

  .ai-ring {
    position: absolute;
    inset: -3px;
    border-radius: 50%;
    border: 2px solid rgba(52, 211, 153, 0.24);
    border-top-color: var(--accent);
    border-right-color: var(--accent);
    pointer-events: none;
    animation: avatar-ring-spin 0.85s linear infinite;
  }

  @keyframes avatar-ring-spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
