/*
 * Thin C shim exposing DPDK inline functions / macros as real
 * `extern "C"` symbols that Rust can call via FFI.
 *
 * Many DPDK "functions" (rx/tx burst, mbuf alloc/free/append,
 * pktmbuf_mtod, pktmbuf_data_len) are actually `static inline` in the
 * headers, so bindgen cannot generate Rust bindings for them.  We wrap
 * each one in a tiny non-inline wrapper below.
 */

#include <rte_ethdev.h>
#include <rte_mbuf.h>
#include <stdint.h>

/* --- Mbuf accessors --- */

const void *ironsbe_pktmbuf_mtod(const struct rte_mbuf *m)
{
    return rte_pktmbuf_mtod(m, const void *);
}

uint16_t ironsbe_pktmbuf_data_len_shim(const struct rte_mbuf *m)
{
    return rte_pktmbuf_data_len(m);
}

struct rte_mbuf *ironsbe_pktmbuf_alloc(struct rte_mempool *pool)
{
    return rte_pktmbuf_alloc(pool);
}

void ironsbe_pktmbuf_free(struct rte_mbuf *m)
{
    rte_pktmbuf_free(m);
}

char *ironsbe_pktmbuf_append(struct rte_mbuf *m, uint16_t len)
{
    return rte_pktmbuf_append(m, len);
}

/* --- Ethdev rx/tx burst --- */

uint16_t ironsbe_eth_rx_burst(uint16_t port_id, uint16_t queue_id,
                               struct rte_mbuf **rx_pkts, uint16_t nb_pkts)
{
    return rte_eth_rx_burst(port_id, queue_id, rx_pkts, nb_pkts);
}

uint16_t ironsbe_eth_tx_burst(uint16_t port_id, uint16_t queue_id,
                               struct rte_mbuf **tx_pkts, uint16_t nb_pkts)
{
    return rte_eth_tx_burst(port_id, queue_id, tx_pkts, nb_pkts);
}
