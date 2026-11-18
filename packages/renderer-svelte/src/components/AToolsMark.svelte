<script lang="ts">
  type Props = {
    size?: "small" | "badge" | "large";
    label?: string;
    circular?: boolean;
    streaming?: boolean;
    aiState?: "idle" | "sending" | "receiving";
  };

  let {
    size = "badge",
    label = "",
    circular = false,
    streaming = false,
    aiState = "idle",
  }: Props = $props();

  const resolvedAiState = $derived<"idle" | "sending" | "receiving">(
    aiState !== "idle" ? aiState : streaming ? "sending" : "idle",
  );
</script>

<span
  class={`atools-mark ${size}`}
  class:circular
  class:streaming={resolvedAiState !== "idle"}
  aria-hidden={!label}
  aria-label={label || undefined}
  role={label ? "img" : undefined}
>
  <svg viewBox="0 0 128 128" aria-hidden="true" focusable="false">
    <path d="M25 99 55 22c1.7-4.4 3.8-6.5 8.1-6.5 4.1 0 6.4 2.1 8.1 6.5L103 99H82L75 80H53l-7 19H25Zm34-36h10l-5-14-5 14Z" />
  </svg>
  {#if resolvedAiState !== "idle"}
    <span class="ai-animation-layer" aria-hidden="true">
      <span class="ai-mask"></span>
      <span class="ai-text">AI</span>
      {#if resolvedAiState === "sending"}
        <span class="ai-ripple-container"></span>
      {:else if resolvedAiState === "receiving"}
        <span class="ai-pulse-container"></span>
      {/if}
    </span>
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

  .ai-animation-layer {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    border-radius: 50%;
    pointer-events: none;
    z-index: 2;
  }

  .ai-mask {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.88);
    z-index: 1;
  }

  :global(:root[data-atools-theme="dark"]) .ai-mask {
    background: rgba(24, 26, 29, 0.86);
  }

  @media (prefers-color-scheme: dark) {
    :global(:root:not([data-atools-theme="light"])) .ai-mask {
      background: rgba(24, 26, 29, 0.86);
    }
  }

  .ai-text {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    font-size: 11px;
    font-weight: 700;
    color: var(--accent-text);
    z-index: 3;
    letter-spacing: 0.4px;
    line-height: 1;
  }

  .ai-ripple-container,
  .ai-pulse-container {
    position: absolute;
    inset: -2px;
    display: inline-block;
    z-index: 2;
  }

  .ai-ripple-container::before,
  .ai-ripple-container::after {
    content: "";
    box-sizing: border-box;
    width: 100%;
    height: 100%;
    border-radius: 50%;
    border: 2px solid var(--accent);
    position: absolute;
    inset: 0;
    animation: animloader-sending 2s linear infinite;
    animation-fill-mode: backwards;
  }

  .ai-ripple-container::after {
    animation-delay: 1s;
  }

  .ai-pulse-container::before,
  .ai-pulse-container::after {
    content: "";
    box-sizing: border-box;
    width: 100%;
    height: 100%;
    border-radius: 50%;
    border: 2px solid var(--accent);
    position: absolute;
    inset: 0;
    animation: animloader-receiving 2s linear infinite;
    animation-fill-mode: backwards;
  }

  .ai-pulse-container::after {
    animation-delay: 1s;
  }

  @keyframes animloader-sending {
    0% {
      transform: scale(0.25);
      opacity: 1;
    }
    100% {
      transform: scale(1.18);
      opacity: 0;
    }
  }

  @keyframes animloader-receiving {
    0% {
      transform: scale(1.18);
      opacity: 0;
    }
    100% {
      transform: scale(0.25);
      opacity: 1;
    }
  }
</style>
