import { Route, Routes } from "react-router-dom";
import Layout from "./layout/Layout";
import Ca from "./pages/Ca";
import Cache from "./pages/Cache";
import Connect from "./pages/Connect";
import Dashboard from "./pages/Dashboard";
import Exclusions from "./pages/Exclusions";
import Health from "./pages/Health";
import Requests from "./pages/Requests";
import Settings from "./pages/Settings";

/** Route table for the SPA. */
export default function App() {
  return (
    <Layout>
      <Routes>
        <Route path="/" element={<Dashboard />} />
        <Route path="/health" element={<Health />} />
        <Route path="/requests" element={<Requests />} />
        <Route path="/exclusions" element={<Exclusions />} />
        <Route path="/cache" element={<Cache />} />
        <Route path="/connect" element={<Connect />} />
        <Route path="/settings" element={<Settings />} />
        <Route path="/ca" element={<Ca />} />
      </Routes>
    </Layout>
  );
}
