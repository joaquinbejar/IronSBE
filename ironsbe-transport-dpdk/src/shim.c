/*
 * Thin C shim exposing DPDK macro-based accessors as real functions
 * that Rust can call via FFI.
 *
 * rte_pktmbuf_mtod and rte_pktmbuf_data_len are inline
 * functions / macros in the DPDK headers, so bindgen cannot generate
 * Rust bindings for them directly.
 */

#include <rte_mbuf.h>
#include <stdint.h>

const void *ironsbe_pktmbuf_mtod(const struct rte_mbuf *m)
{
    return rte_pktmbuf_mtod(m, const void *);
}

uint16_t ironsbe_pktmbuf_data_len_shim(const struct rte_mbuf *m)
{
    return rte_pktmbuf_data_len(m);
}
